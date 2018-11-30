use crate::errors::{ScrapeError, ScrapeResult, ServiceError, ServiceErrorKind, ServiceResult};
use crate::service::session::HttpSession;
use crate::service::{
    Contest, DownloadProps, PrintTargets as _PrintTargets, ProblemNameConversion, RestoreProps,
    Service, SessionProps, SubmitProps, UserNameAndPassword,
};
use crate::terminal::{Term, WriteAnsi as _WriteAnsi};
use crate::testsuite::{InteractiveSuite, SimpleSuite, TestSuite};
use crate::util::std_unstable::RemoveItem_ as _RemoveItem_;

use chrono::{DateTime, Local, Utc};
use failure::ResultExt as _ResultExt;
use maplit::hashmap;
use once_cell::sync::Lazy;
use regex::Regex;
use reqwest::{header, StatusCode};
use select::document::Document;
use select::predicate::{Predicate, Text};
use tokio::runtime::Runtime;

use std::collections::{BTreeMap, HashMap};
use std::io::Write as _Write;
use std::rc::Rc;
use std::str::FromStr;
use std::time::Duration;
use std::vec;

/// Logins to "beta.atcoder.jp".
pub(crate) fn login(sess_props: SessionProps<impl Term>) -> ServiceResult<()> {
    Atcoder::try_new(sess_props)?.login_if_not(true)
}

/// Participates in a `contest_name`.
pub(crate) fn participate(
    contest_name: &str,
    sess_props: SessionProps<impl Term>,
) -> ServiceResult<()> {
    Atcoder::try_new(sess_props)?.register_explicitly(&AtcoderContest::new(contest_name))
}

/// Accesses to pages of the problems and extracts pairs of sample input/output
/// from them.
pub(crate) fn download(
    mut sess_props: SessionProps<impl Term>,
    download_props: DownloadProps<String>,
) -> ServiceResult<()> {
    let download_props = download_props.convert_contest_and_problems(ProblemNameConversion::Upper);
    download_props.print_targets(sess_props.term.stdout())?;
    Atcoder::try_new(sess_props)?.download(&download_props)
}

/// Downloads submitted source codes.
pub(crate) fn restore(
    mut sess_props: SessionProps<impl Term>,
    restore_props: RestoreProps<String>,
) -> ServiceResult<()> {
    let restore_props = restore_props.convert_contest_and_problems(ProblemNameConversion::Upper);
    restore_props.print_targets(sess_props.term.stdout())?;
    Atcoder::try_new(sess_props)?.restore(&restore_props)
}

/// Submits a source code.
pub(crate) fn submit(
    mut sess_props: SessionProps<impl Term>,
    submit_props: SubmitProps<String>,
) -> ServiceResult<()> {
    let submit_props = submit_props.convert_contest_and_problem(ProblemNameConversion::Upper);
    submit_props.print_targets(sess_props.term.stdout())?;
    Atcoder::try_new(sess_props)?.submit(&submit_props)
}

pub(self) struct Atcoder<T: Term> {
    term: T,
    session: HttpSession,
    runtime: Runtime,
    credentials: UserNameAndPassword,
}

impl<T: Term> Service for Atcoder<T> {
    type Term = T;

    fn requirements(&mut self) -> (&mut T, &mut HttpSession, &mut Runtime) {
        (&mut self.term, &mut self.session, &mut self.runtime)
    }
}

impl<T: Term> Atcoder<T> {
    fn try_new(mut sess_props: SessionProps<T>) -> ServiceResult<Self> {
        let credentials = sess_props.credentials.atcoder.clone();
        let mut runtime = Runtime::new()?;
        let session = sess_props.start_session(&mut runtime)?;
        Ok(Self {
            term: sess_props.term,
            session,
            runtime,
            credentials,
        })
    }

    fn login_if_not(&mut self, eprints_message_if_already_logged_in: bool) -> ServiceResult<()> {
        if self.session.has_cookie() {
            let status = self.get("/settings").acceptable(&[200, 302]).status()?;
            if status == StatusCode::OK {
                if eprints_message_if_already_logged_in {
                    writeln!(self.stderr(), "Already logged in.")?;
                    self.stderr().flush()?;
                }
                return Ok(());
            }
        }

        while !self.try_logging_in()? {
            writeln!(self.stderr(), "Failed to login. Try again.")?;
            self.stderr().flush()?;
            self.session.clear_cookies()?;
        }
        Ok(())
    }

    fn try_logging_in(&mut self) -> ServiceResult<bool> {
        let token = self.get("/login").recv_html()?.extract_csrf_token()?;
        let (username, password) = match self.credentials.clone() {
            UserNameAndPassword::Some(username, password) => (username.clone(), password.clone()),
            UserNameAndPassword::None => (
                Rc::new(self.term.prompt_reply_stderr("Username: ")?),
                Rc::new(self.term.prompt_password_stderr("Password: ")?),
            ),
        };
        let payload = hashmap!(
            "username" => username.as_str(),
            "password" => password.as_str(),
            "csrf_token" => token.as_str(),
        );
        self.post("/login").send_form(&payload)?;
        let status = self.get("/settings").acceptable(&[200, 302]).status()?;
        let success = status == StatusCode::OK;
        if success {
            writeln!(self.stdout(), "Successfully logged in.")?;
            self.stdout().flush()?;
        } else if self.credentials.is_some() {
            return Err(ServiceErrorKind::LoginOnTest.into());
        }
        Ok(success)
    }

    fn register_explicitly(&mut self, contest: &AtcoderContest) -> ServiceResult<()> {
        self.register_if_active_or_explicit(contest, true)
    }

    fn fetch_tasks_page(&mut self, contest: &AtcoderContest) -> ServiceResult<Document> {
        let res = self
            .get(&contest.url_tasks())
            .acceptable(&[200, 302, 404])
            .send()?;
        if res.status() == 200 {
            res.document(&mut self.runtime)
        } else {
            self.register_if_active_or_explicit(contest, false)?;
            self.get(&contest.url_tasks()).recv_html()
        }
    }

    fn register_if_active_or_explicit(
        &mut self,
        contest: &AtcoderContest,
        explicit: bool,
    ) -> ServiceResult<()> {
        let res = self
            .get(&contest.url_top())
            .acceptable(&[200, 302])
            .send()?;
        if res.status() == StatusCode::FOUND {
            return Err(ServiceErrorKind::ContestNotFound(contest.to_string()).into());
        }
        let page = res.document(&mut self.runtime)?;
        let duration = page.extract_contest_duration()?;
        let status = duration.check_current_status(contest.to_string());
        if !explicit {
            status.raise_if_not_begun()?;
        }
        if explicit || *contest == AtcoderContest::Practice || status.is_active() {
            self.login_if_not(false)?;
            let csrf_token = self
                .get(&contest.url_top())
                .recv_html()?
                .extract_csrf_token()?;
            let url = contest.url_register();
            let payload = hashmap!("csrf_token" => csrf_token);
            self.post(&url).send_form(&payload)?;
        }
        Ok(())
    }

    fn download(&mut self, prop: &DownloadProps<AtcoderContest>) -> ServiceResult<()> {
        let DownloadProps {
            contest,
            problems,
            destinations,
            open_browser,
        } = prop;
        let outputs = self
            .fetch_tasks_page(contest)?
            .extract_task_urls_with_names()?
            .into_iter()
            .filter(|(name, _)| match problems.as_ref() {
                None => true,
                Some(problems) => problems.iter().any(|p| p == name),
            }).map(|(name, url)| -> ServiceResult<_> {
                let suite = match contest.preset_suite() {
                    Some(suite) => suite,
                    None => self.get(&url).recv_html()?.extract_as_suite()?,
                };
                let path = destinations.scraping(&name)?;
                Ok((url, name, suite, path))
            }).collect::<ServiceResult<Vec<_>>>()?;
        let mut not_found = match problems.as_ref() {
            None => vec![],
            Some(problems) => problems.iter().collect(),
        };
        for (_, name, suite, path) in &outputs {
            suite.save(&name, path, self.stdout())?;
            not_found.remove_item_(&name);
        }
        self.stdout().flush()?;
        if !not_found.is_empty() {
            self.stderr()
                .with_reset(|o| writeln!(o.fg(11)?, "Not found: {:?}", not_found))?;
            self.stderr().flush()?;
        }
        if *open_browser {
            self.open_in_browser(&contest.url_submissions_me(1))?;
            for (url, _, _, _) in &outputs {
                self.open_in_browser(url)?;
            }
        }
        Ok(())
    }

    fn restore(&mut self, prop: &RestoreProps<AtcoderContest>) -> ServiceResult<()> {
        fn collect_urls(
            detail_urls: &mut HashMap<(String, String), String>,
            submissions: vec::IntoIter<Submission>,
        ) {
            for submission in submissions {
                let key = (submission.task_name, submission.lang_name);
                if detail_urls.get(&key).is_none() {
                    detail_urls.insert(key, submission.detail_url);
                }
            }
        }

        let RestoreProps {
            contest,
            problems,
            src_paths,
            replacers,
        } = prop;
        let first_page = self.get(&contest.url_submissions_me(1)).recv_html()?;
        let (submissions, num_pages) = first_page.extract_submissions()?;
        let mut detail_urls = HashMap::new();
        collect_urls(&mut detail_urls, submissions);
        for i in 2..=num_pages {
            let page = self.get(&contest.url_submissions_me(i)).recv_html()?;
            let (submission, _) = page.extract_submissions()?;
            collect_urls(&mut detail_urls, submission);
        }
        let mut results = vec![];
        for ((task_name, lang_name), detail_url) in detail_urls {
            if problems.is_some() && !problems.as_ref().unwrap().iter().any(|p| p == &task_name) {
                continue;
            }
            let code = self
                .get(&detail_url)
                .recv_html()?
                .extract_submitted_code()?;
            let lang_id = first_page.extract_lang_id(&lang_name)?;
            if let Some(path_template) = src_paths.get(lang_id.as_str()) {
                let path = path_template.expand(&task_name.to_lowercase())?;
                let code = match replacers.get(lang_id.as_str()) {
                    Some(replacer) => {
                        replacer.replace_from_submission_to_local(&task_name, &code)?
                    }
                    None => code,
                };
                crate::fs::write(&path, code.as_bytes())?;
                results.push((task_name, lang_name, lang_id, path));
            } else {
                self.stderr().with_reset(|o| {
                    writeln!(o.fg(11)?, "Ignoring {:?} (id: {})", lang_name, lang_id)
                })?;
                self.stderr().flush()?;
            }
        }
        let mut not_found = match problems.as_ref() {
            None => vec![],
            Some(problems) => problems.iter().collect(),
        };
        for (task_name, lang_name, lang_id, path) in &results {
            writeln!(
                self.stdout(),
                "{} - {:?} (id: {}): Saved to {}",
                task_name,
                lang_name,
                lang_id,
                path.display()
            )?;
            not_found.remove_item_(&task_name);
        }
        if !not_found.is_empty() {
            self.stderr()
                .with_reset(|o| writeln!(o.fg(11)?, "Not found: {:?}", not_found))?;
            self.stderr().flush()?;
        }
        let stdout = self.stdout();
        writeln!(stdout, "Saved {}.", plural!(results.len(), "file", "files"))?;
        stdout.flush()?;
        Ok(())
    }

    fn submit(&mut self, props: &SubmitProps<AtcoderContest>) -> ServiceResult<()> {
        let SubmitProps {
            contest,
            problem,
            lang_id,
            src_path,
            replacer,
            open_browser,
            skip_checking_if_accepted,
        } = props;
        let tasks_page = self.fetch_tasks_page(&contest)?;
        let checks_if_accepted =
            !skip_checking_if_accepted && *contest != AtcoderContest::Practice && {
                let duration = tasks_page.extract_contest_duration()?;
                let status = duration.check_current_status(contest.to_string());
                status.raise_if_not_begun()?;
                status.is_active()
            };
        for (name, url) in tasks_page.extract_task_urls_with_names()? {
            if &name == problem {
                let task_screen_name = {
                    static SCREEN_NAME: Lazy<Regex> =
                        lazy_regex!(r"\A/contests/[a-z0-9_\-]+/tasks/([a-z0-9_]+)/?\z$");
                    if let Some(caps) = SCREEN_NAME.captures(&url) {
                        caps[1].to_owned()
                    } else {
                        break;
                    }
                };
                if checks_if_accepted {
                    let (mut submissions, num_pages) = self
                        .get(&contest.url_submissions_me(1))
                        .recv_html()?
                        .extract_submissions()?;
                    if submissions.any(|s| s.task_screen_name == task_screen_name && s.is_ac) {
                        return Err(ServiceErrorKind::AlreadyAccepted.into());
                    }
                    for i in 2..=num_pages {
                        if self
                            .get(&contest.url_submissions_me(i))
                            .recv_html()?
                            .extract_submissions()?
                            .0
                            .any(|s| s.task_screen_name == task_screen_name && s.is_ac)
                        {
                            return Err(ServiceErrorKind::AlreadyAccepted.into());
                        }
                    }
                }

                let source_code = crate::fs::read_to_string(src_path)?;
                let source_code = match replacer {
                    Some(replacer) => {
                        replacer.replace_from_local_to_submission(&problem, &source_code)?
                    }
                    None => source_code,
                };
                let csrf_token = self.get(&url).recv_html()?.extract_csrf_token()?;
                let url = contest.url_submit();
                let payload = hashmap!(
                    "data.TaskScreenName" => &task_screen_name,
                    "data.LanguageId" => lang_id,
                    "sourceCode" => &source_code,
                    "csrf_token" => &csrf_token,
                );

                let error = |status: StatusCode, location: Option<String>| -> _ {
                    ServiceError::from(ServiceErrorKind::SubmissionRejected(
                        lang_id.to_owned(),
                        source_code.len(),
                        status,
                        location,
                    ))
                };

                match self.post(&url).send_form(&payload) {
                    Ok(res) => {
                        let location = res
                            .headers()
                            .get(header::LOCATION)
                            .ok_or_else(|| error(res.status(), None))?;
                        let location = location
                            .to_str()
                            .with_context(|_| ServiceErrorKind::ReadHeader(header::LOCATION))?;
                        if !(location.starts_with("/contests/")
                            && location.ends_with("/submissions/me"))
                        {
                            return Err(error(res.status(), Some(location.to_owned())));
                        }
                    }
                    Err(err) => {
                        if let ServiceError::Context(ctx) = &err {
                            if let ServiceErrorKind::UnexpectedStatusCode(_, status, _) =
                                ctx.get_context()
                            {
                                return Err(error(*status, None));
                            }
                        }
                        return Err(err);
                    }
                }

                if *open_browser {
                    self.open_in_browser(&contest.url_submissions_me(1))?;
                }
                return Ok(());
            }
        }
        Err(ServiceErrorKind::NoSuchProblem(problem.clone()).into())
    }
}

#[derive(Clone, PartialEq, Eq, derive_more::Display)]
enum AtcoderContest {
    #[display(fmt = "practice contest")]
    Practice,
    #[display(fmt = "AtCoder Programming Guide for beginners")]
    Apg4b,
    #[display(fmt = "ARC{:>03}", _0)]
    Arc(u32),
    #[display(fmt = "ABC{:>03}", _0)]
    Abc(u32),
    #[display(fmt = "AGC{:>03}", _0)]
    Agc(u32),
    #[display(fmt = "ATC{:>03}", _0)]
    Atc(u32),
    #[display(fmt = "APC{:>03}", _0)]
    Apc(u32),
    #[display(fmt = "Chokudai SpeedRun {:>03}", _0)]
    ChokudaiS(u32),
    #[display(fmt = "{}", _0)]
    Other(String),
}

impl AtcoderContest {
    fn new(s: &str) -> Self {
        static NAME: Lazy<Regex> = lazy_regex!(r"\A\s*([a-zA-Z_]+)(\d{3})\s*\z");
        if let Some(caps) = NAME.captures(s) {
            let name = caps[1].to_lowercase();
            let number = caps[2].parse::<u32>().unwrap_or(0);
            if name == "abc" {
                return AtcoderContest::Abc(number);
            } else if name == "arc" {
                return AtcoderContest::Arc(number);
            } else if name == "agc" {
                return AtcoderContest::Agc(number);
            } else if name == "atc" {
                return AtcoderContest::Atc(number);
            } else if name == "apc" {
                return AtcoderContest::Apc(number);
            } else if name == "chokudai_s" || name == "chokudais" {
                return AtcoderContest::ChokudaiS(number);
            }
        }
        if s == "practice" {
            AtcoderContest::Practice
        } else if s == "apg4b" {
            AtcoderContest::Apg4b
        } else {
            AtcoderContest::Other(s.to_owned())
        }
    }

    fn url_top(&self) -> String {
        static BASE: &'static str = "/contests/";
        match self {
            AtcoderContest::Practice => format!("{}practice", BASE),
            AtcoderContest::Apg4b => format!("{}apg4b", BASE),
            AtcoderContest::Abc(n) => format!("{}abc{:>03}", BASE, n),
            AtcoderContest::Arc(n) => format!("{}arc{:>03}", BASE, n),
            AtcoderContest::Agc(n) => format!("{}agc{:>03}", BASE, n),
            AtcoderContest::Atc(n) => format!("{}atc{:>03}", BASE, n),
            AtcoderContest::Apc(n) => format!("{}apc{:>03}", BASE, n),
            AtcoderContest::ChokudaiS(n) => format!("{}chokudai_s{:>03}", BASE, n),
            AtcoderContest::Other(s) => format!("{}{}", BASE, s),
        }
    }

    fn url_tasks(&self) -> String {
        format!("{}/tasks", self.url_top())
    }

    fn url_register(&self) -> String {
        format!("{}/register", self.url_top())
    }

    fn url_submit(&self) -> String {
        format!("{}/submit", self.url_top())
    }

    fn url_submissions_me(&self, page: u32) -> String {
        format!("{}/submissions/me?page={}", self.url_top(), page)
    }

    fn preset_suite(&self) -> Option<TestSuite> {
        match self {
            AtcoderContest::Arc(19) => Some(InteractiveSuite::new(Duration::from_secs(2))),
            AtcoderContest::Arc(21) => Some(InteractiveSuite::new(Duration::from_secs(4))),
            _ => None,
        }.map(Into::into)
    }
}

impl Contest for AtcoderContest {
    fn from_string(s: String) -> Self {
        Self::new(&s)
    }
}

#[derive(Debug)]
enum ContestStatus {
    Finished,
    Active,
    NotBegun(String, DateTime<Local>),
}

impl ContestStatus {
    fn is_active(&self) -> bool {
        match *self {
            ContestStatus::Active => true,
            _ => false,
        }
    }

    fn raise_if_not_begun(&self) -> ServiceResult<()> {
        if let ContestStatus::NotBegun(s, t) = self {
            Err(ServiceErrorKind::ContestNotBegun(s.clone(), *t).into())
        } else {
            Ok(())
        }
    }
}

struct ContestDuration(DateTime<Utc>, DateTime<Utc>);

impl ContestDuration {
    fn check_current_status(&self, contest_name: String) -> ContestStatus {
        let now = Utc::now();
        if now < self.0 {
            ContestStatus::NotBegun(contest_name, self.0.with_timezone(&Local))
        } else if now > self.1 {
            ContestStatus::Finished
        } else {
            ContestStatus::Active
        }
    }
}

struct Submission {
    task_name: String,
    task_screen_name: String,
    lang_name: String,
    detail_url: String,
    is_ac: bool,
}

trait Extract {
    fn extract_csrf_token(&self) -> ScrapeResult<String>;
    fn extract_task_urls_with_names(&self) -> ScrapeResult<Vec<(String, String)>>;
    fn extract_as_suite(&self) -> ScrapeResult<TestSuite>;
    fn extract_contest_duration(&self) -> ScrapeResult<ContestDuration>;
    fn extract_submissions(&self) -> ScrapeResult<(vec::IntoIter<Submission>, u32)>;
    fn extract_submitted_code(&self) -> ScrapeResult<String>;
    fn extract_lang_id(&self, lang_name: &str) -> ScrapeResult<String>;
}

impl Extract for Document {
    fn extract_csrf_token(&self) -> ScrapeResult<String> {
        self.find(selector!("[name=\"csrf_token\"]"))
            .next()
            .and_then(|node| node.attr("value").map(ToOwned::to_owned))
            .filter(|token| !token.is_empty())
            .ok_or_else(ScrapeError::new)
    }

    fn extract_task_urls_with_names(&self) -> ScrapeResult<Vec<(String, String)>> {
        let extract = || {
            let mut names_and_pathes = vec![];
            for node in self.find(selector!(
                "#main-container > div.row > div.col-sm-12 > div.panel > table.table > tbody > tr",
            )) {
                let node = node.find(selector!("td.text-center > a")).next()?;
                let url = node.attr("href")?.to_owned();
                let name = node.find(Text).next()?.text();
                names_and_pathes.push((name, url));
            }
            if names_and_pathes.is_empty() {
                None
            } else {
                Some(names_and_pathes)
            }
        };
        extract().ok_or_else(ScrapeError::new)
    }

    fn extract_as_suite(&self) -> ScrapeResult<TestSuite> {
        enum Samples {
            Simple(Vec<(String, String)>),
            Interactive,
        }

        fn extract_samples(this: &Document) -> Option<Samples> {
            // Interactive problems:
            // - ARC070/F https://beta.atcoder.jp/contests/arc070/tasks/arc070_d
            // - ARC078/E https://beta.atcoder.jp/contests/arc078/tasks/arc078_c
            // - APC001/C https://beta.atcoder.jp/contests/apc001/tasks/apc001_c
            // TODO:
            // - https://beta.atcoder.jp/contests/arc019/tasks/arc019_4 (interactive)
            // - https://beta.atcoder.jp/contests/arc021/tasks/arc021_4 (interactive)
            // - https://beta.atcoder.jp/contests/cf17-final-open/tasks/cf17_final_f
            // - https://beta.atcoder.jp/contests/jag2016-domestic/tasks
            // - https://beta.atcoder.jp/contests/chokudai001/tasks/chokudai_001_a

            static IN_JA: Lazy<Regex> = lazy_regex!(r"\A[\s\n]*入力例\s*(\d{1,2})+[.\n]*\z");
            static OUT_JA: Lazy<Regex> = lazy_regex!(r"\A[\s\n]*出力例\s*(\d{1,2})+[.\n]*\z");
            static IN_EN: Lazy<Regex> = lazy_regex!(r"\ASample Input\s?([0-9]{1,2}).*\z");
            static OUT_EN: Lazy<Regex> = lazy_regex!(r"\ASample Output\s?([0-9]{1,2}).*\z");

            // Current style (Japanese)
            let p1_head =
                selector!("#task-statement > span.lang > span.lang-ja > div.part > section > h3");
            let p1_content =
                selector!("#task-statement > span.lang > span.lang-ja > div.part > section > pre");
            // Current style (English)
            let p2_head =
                selector!("#task-statement > span.lang > span.lang-en > div.part > section > h3");
            let p2_content =
                selector!("#task-statement>span.lang>span.lang-en>div.part>section>pre");
            // ARC019..ARC057 \ {ARC019/C, ARC046/D, ARC050, ARC052/{A, C}, ARC053, ARC055},
            // ABC007..ABC040 \ {ABC036}, ATC001, ATC002
            let p3_head = selector!("#task-statement > div.part > section > h3");
            let p3_content = selector!("#task-statement > div.part > section > pre");
            // ARC002..ARC018, ARC019/C, ABC001..ABC006
            let p4_head = selector!("#task-statement > div.part > h3,pre");
            let p4_content = selector!("#task-statement > div.part > section > pre");
            // ARC001, dwacon2018-final/{A, B}
            let p5_head = selector!("#task-statement > h3,pre");
            let p5_content = selector!("#task-statement > section > pre");
            // ARC046/D, ARC050, ARC052/{A, C}, ARC053, ARC055, ABC036, ABC041
            let p6_head = selector!("#task-statement > section > h3");
            let p6_content = selector!("#task-statement > section > pre");
            // ABC034
            let p7_head = selector!("#task-statement > span.lang > span.lang-ja > section > h3");
            let p7_content =
                selector!("#task-statement > span.lang > span.lang-ja > section > pre");
            // practice contest (Japanese)
            let p8_head = selector!("#task-statement > span.lang > span.lang-ja > div.part > h3");
            let p8_content =
                selector!("#task-statement > span.lang > span.lang-ja > div.part > section > pre");

            try_extract_samples(this, p1_head, p1_content, &IN_JA, &OUT_JA)
                .or_else(|| try_extract_samples(this, p2_head, p2_content, &IN_EN, &OUT_EN))
                .or_else(|| try_extract_samples(this, p3_head, p3_content, &IN_JA, &OUT_JA))
                .or_else(|| try_extract_samples(this, p4_head, p4_content, &IN_JA, &OUT_JA))
                .or_else(|| try_extract_samples(this, p5_head, p5_content, &IN_JA, &OUT_JA))
                .or_else(|| try_extract_samples(this, p6_head, p6_content, &IN_JA, &OUT_JA))
                .or_else(|| try_extract_samples(this, p7_head, p7_content, &IN_JA, &OUT_JA))
                .or_else(|| try_extract_samples(this, p8_head, p8_content, &IN_JA, &OUT_JA))
        }

        fn try_extract_samples(
            this: &Document,
            predicate_for_header: impl Predicate,
            predicate_for_content: impl Predicate,
            re_input: &'static Regex,
            re_output: &'static Regex,
        ) -> Option<Samples> {
            for strong in this.find(selector!("#task-statement strong")) {
                let text = strong.text();
                for word in &["インタラクティブ", "Interactive"] {
                    if text.find(word).is_some() {
                        return Some(Samples::Interactive);
                    }
                }
            }
            let mut inputs = BTreeMap::<usize, _>::new();
            let mut outputs = BTreeMap::<usize, _>::new();
            let mut next = None;
            for node in this.find(predicate_for_header.or(predicate_for_content)) {
                if node.name() == Some("h3") {
                    let text = node.text();
                    if let Some(caps) = re_input.captures(&text) {
                        next = Some((true, parse_zenkaku(&caps[1]).ok()?));
                    } else if let Some(caps) = re_output.captures(&text) {
                        next = Some((false, parse_zenkaku(&caps[1]).ok()?));
                    }
                } else if [Some("pre"), Some("section")].contains(&node.name()) {
                    if let Some((is_input, n)) = next {
                        if is_input {
                            inputs.insert(n, node.text());
                        } else {
                            outputs.insert(n, node.text());
                        }
                    }
                    next = None;
                }
            }
            let mut samples = vec![];
            for (i, input) in inputs {
                if let Some(output) = outputs.remove(&i) {
                    samples.push((input, output));
                }
            }

            for (input, output) in &mut samples {
                for s in &mut [input, output] {
                    if !s.ends_with('\n') {
                        s.push('\n');
                    }
                    if !is_valid_text(s) {
                        return None;
                    }
                }
            }

            if samples.is_empty() {
                None
            } else {
                Some(Samples::Simple(samples))
            }
        }

        fn parse_zenkaku<T: FromStr>(s: &str) -> Result<T, T::Err> {
            match s.parse() {
                Ok(v) => Ok(v),
                Err(e) => if s.chars().all(|c| '０' <= c && c <= '９') {
                    s.chars()
                        .map(|c| {
                            char::from((u32::from(c) - u32::from('０') + u32::from('0')) as u8)
                        }).collect::<String>()
                        .parse()
                } else {
                    Err(e)
                },
            }
        }

        fn is_valid_text(s: &str) -> bool {
            s == "\n" || ![' ', '\n'].iter().any(|&c| s.starts_with(c)) && s
                .chars()
                .all(|c| c.is_ascii() && (c.is_ascii_whitespace() == [' ', '\n'].contains(&c)))
        }

        fn extract_timelimit(this: &Document) -> Option<Duration> {
            static TIMELIMIT: Lazy<Regex> =
                lazy_regex!(r"\A\D*([0-9]{1,9})(\.[0-9]{1,3})?\s*(m)?sec.*\z");
            let text = this
                .find(selector!("#main-container > div.row > div.col-sm-12 > p").child(Text))
                .next()?
                .text();
            let caps = TIMELIMIT.captures(&text)?;
            let (mut b, mut e) = (caps[1].parse::<u64>().unwrap(), 0);
            if let Some(cap) = caps.get(2) {
                let n = cap.as_str().len() as u32 - 1;
                b *= 10u64.pow(n);
                b += cap.as_str()[1..].parse::<u64>().ok()?;
                e -= n as i32;
            }
            if caps.get(3).is_none() {
                e += 3;
            }
            let timelimit = if e < 0 {
                b / 10u64.pow(-e as u32)
            } else {
                b * 10u64.pow(e as u32)
            };
            Some(Duration::from_millis(timelimit))
        }

        let timelimit = extract_timelimit(self).ok_or_else(ScrapeError::new)?;
        if timelimit == Duration::from_millis(0) {
            return Ok(TestSuite::Unsubmittable);
        }
        match extract_samples(self) {
            None => Ok(SimpleSuite::new(timelimit).into()),
            Some(Samples::Simple(samples)) => Ok(SimpleSuite::new(timelimit).cases(samples).into()),
            Some(Samples::Interactive) => Ok(InteractiveSuite::new(timelimit).into()),
        }
    }

    fn extract_contest_duration(&self) -> ScrapeResult<ContestDuration> {
        fn extract(this: &Document) -> Option<(DateTime<Utc>, DateTime<Utc>)> {
            static FORMAT: &'static str = "%F %T%z";
            let t1 = this.find(selector!("time").child(Text)).nth(0)?.text();
            let t2 = this.find(selector!("time").child(Text)).nth(1)?.text();
            let t1 = DateTime::parse_from_str(&t1, FORMAT).ok()?;
            let t2 = DateTime::parse_from_str(&t2, FORMAT).ok()?;
            Some((t1.with_timezone(&Utc), t2.with_timezone(&Utc)))
        }

        match extract(self) {
            Some((t1, t2)) => Ok(ContestDuration(t1, t2)),
            None => Err(ScrapeError::new()),
        }
    }

    fn extract_submissions(&self) -> ScrapeResult<(vec::IntoIter<Submission>, u32)> {
        let extract = || {
            let num_pages = self
                .find(selector!(
                    "#main-container > div.row > div.text-center > ul.pagination > li",
                )).count() as u32;
            let mut submissions = vec![];
            let pred = selector!(
                "#main-container > div.row > div.col-sm-12 > div.panel-submission
                 > div.table-responsive > table.table > tbody > tr",
            );
            for tr in self.find(pred) {
                let (task_name, task_screen_name) = {
                    static SCREEN_NAME: Lazy<Regex> = lazy_regex!(r"\A(\w+).*\z");
                    static TASK_SCREEN_NAME: Lazy<Regex> =
                        lazy_regex!(r"\A/contests/[\w-]+/tasks/([\w-]+)\z");
                    let a = tr.find(selector!("td > a")).nth(0)?;
                    let task_full_name = a.find(Text).next()?.text();
                    let task_name = SCREEN_NAME.captures(&task_full_name)?[1].to_owned();
                    let task_url = a.attr("href")?;
                    let task_screen_name = TASK_SCREEN_NAME.captures(task_url)?[1].to_owned();
                    (task_name, task_screen_name)
                };
                let lang_name = tr.find(selector!("td")).nth(3)?.find(Text).next()?.text();
                let is_ac = {
                    let status = tr.find(selector!("td > span").child(Text)).nth(0)?.text();
                    status == "AC"
                };
                let detail_url = tr
                    .find(selector!("td.text-center > a"))
                    .flat_map(|a| -> Option<String> {
                        let text = a.find(Text).next()?.text();
                        if text != "詳細" && text != "Detail" {
                            return None;
                        }
                        a.attr("href").map(ToOwned::to_owned)
                    }).next()?;
                submissions.push(Submission {
                    task_name,
                    task_screen_name,
                    lang_name,
                    detail_url,
                    is_ac,
                })
            }
            Some((submissions.into_iter(), num_pages))
        };
        extract().ok_or_else(ScrapeError::new)
    }

    fn extract_submitted_code(&self) -> ScrapeResult<String> {
        let submission_code = self
            .find(selector!("#submission-code"))
            .next()
            .ok_or_else(ScrapeError::new)?;
        Ok(submission_code
            .find(Text)
            .next()
            .map(|t| t.text())
            .unwrap_or_else(|| "".to_owned()))
    }

    fn extract_lang_id(&self, lang_name: &str) -> ScrapeResult<String> {
        for option in self.find(selector!("#select-language > option")) {
            if let Some(text) = option.find(Text).next().map(|n| n.text()) {
                if text == lang_name {
                    return option
                        .attr("value")
                        .map(ToOwned::to_owned)
                        .ok_or_else(ScrapeError::new);
                }
            }
        }
        Err(ScrapeError::new())
    }
}

#[cfg(test)]
mod tests {
    use crate::errors::ServiceResult;
    use crate::service::atcoder::{Atcoder, AtcoderContest, Extract as _Extract};
    use crate::service::session::{HttpSession, UrlBase};
    use crate::service::{self, Service as _Service, UserNameAndPassword};
    use crate::terminal::{Term, TermImpl};
    use crate::testsuite::{SimpleSuite, TestSuite};

    use tokio::runtime::Runtime;
    use url::Host;

    use std::time::Duration;

    #[test]
    fn it_extracts_task_urls_from_arc001() {
        let _ = env_logger::try_init();
        let mut atcoder = start().unwrap();
        let page = atcoder
            .fetch_tasks_page(&AtcoderContest::new("arc001"))
            .unwrap();
        let urls_and_names = page.extract_task_urls_with_names().unwrap();
        static EXPECTED: &[(&str, &str)] = &[
            ("A", "/contests/arc001/tasks/arc001_1"),
            ("B", "/contests/arc001/tasks/arc001_2"),
            ("C", "/contests/arc001/tasks/arc001_3"),
            ("D", "/contests/arc001/tasks/arc001_4"),
        ];
        assert_eq!(EXPECTED.len(), urls_and_names.len());
        for ((actual_name, actual_url), &(expected_name, expected_url)) in
            urls_and_names.into_iter().zip(EXPECTED)
        {
            assert_eq!(expected_name, actual_name);
            assert_eq!(expected_url, actual_url);
        }
    }

    #[test]
    fn it_extracts_a_timelimit_from_apg4b_b() {
        let _ = env_logger::try_init();
        let mut atcoder = start().unwrap();
        let page = atcoder
            .get("/contests/apg4b/tasks/APG4b_b")
            .recv_html()
            .unwrap();
        match page.extract_as_suite().unwrap() {
            TestSuite::Unsubmittable => {}
            suite => panic!("Got {:?}", suite),
        }
    }

    #[rustfmt::skip]
    #[test]
    fn it_extracts_timelimits_and_sample_cases_from_arc001() {
        static A: &[(&str, &str)] = &[
            ("9\n131142143\n", "4 1\n"),
            ("20\n12341234123412341234\n", "5 5\n"),
            ("4\n1111\n", "4 0\n"),
        ];
        static B: &[(&str, &str)] = &[("7 34\n", "5\n"), ("19 28\n", "2\n"), ("10 10\n", "0\n")];
        static C: &[(&str, &str)] = &[
            ("........\n........\n.......Q\n........\n..Q.....\n........\n.Q......\n........\n",
             "Q.......\n....Q...\n.......Q\n.....Q..\n..Q.....\n......Q.\n.Q......\n...Q....\n"),
            (".....Q..\n.Q......\n........\n........\n........\nQ.......\n........\n........\n",
             "No Answer\n"),
        ];
        static D: &[(&str, &str)] = &[
            ("7\n3 3\n2 5\n4 6\n2 3\n3 6\n3 4\n4 6\n2 5\n1 5\n", "8.22677276241436\n"),
            ("5\n3 3\n0 5\n0 5\n0 5\n0 5\n0 5\n0 5\n", "5\n"),
        ];
        static EXPECTED: Expected = &[
            ("A", "/contests/arc001/tasks/arc001_1", 2000, A),
            ("B", "/contests/arc001/tasks/arc001_2", 2000, B),
            ("C", "/contests/arc001/tasks/arc001_3", 2000, C),
            ("D", "/contests/arc001/tasks/arc001_4", 2000, D),
        ];
        let _ = env_logger::try_init();
        test_sample_extraction("arc001", EXPECTED);
    }

    #[test]
    fn it_extracts_timelimits_and_sample_cases_from_arc002() {
        static A: &[(&str, &str)] = &[
            ("1001\n", "NO\n"),
            ("2012\n", "YES\n"),
            ("2100\n", "NO\n"),
            ("2000\n", "YES\n"),
        ];
        static B: &[(&str, &str)] = &[
            ("2012/05/02\n", "2013/01/01\n"),
            ("2020/05/02\n", "2020/05/02\n"),
            ("2088/02/28\n", "2088/02/29\n"),
        ];
        static C: &[(&str, &str)] = &[
            ("4\nABXY\n", "2\n"),
            ("13\nABABABABXBXBX\n", "7\n"),
            ("8\nAABBAABB\n", "4\n"),
        ];
        static D: &[(&str, &str)] = &[
            ("3 10\n..o.o.xxx.\n...o.xo.x.\no.xxo..x..\n", "o\n"),
            ("3 5\n..x..\n.o...\n...x.\n", "x\n"),
        ];
        static EXPECTED: Expected = &[
            ("A", "/contests/arc002/tasks/arc002_1", 2000, A),
            ("B", "/contests/arc002/tasks/arc002_2", 2000, B),
            ("C", "/contests/arc002/tasks/arc002_3", 2000, C),
            ("D", "/contests/arc002/tasks/arc002_4", 2000, D),
        ];
        let _ = env_logger::try_init();
        test_sample_extraction("arc002", EXPECTED);
    }

    #[rustfmt::skip]
    #[test]
    fn it_extracts_timelimits_and_sample_cases_from_arc019() {
        static A: &[(&str, &str)] = &[
            ("1Z0\n", "120\n"),
            ("4ZD6O\n", "42060\n"),
            ("BI9Z\n", "8192\n"),
        ];
        static B: &[(&str, &str)] = &[
            ("ARC\n", "73\n"),
            ("S\n", "0\n"),
            ("NOLEMONNOMELON\n", "350\n"),
        ];
        static C: &[(&str, &str)] = &[
            ("5 7 3\nGET..ET\n..T....\n.TEST..\n.E.T.ET\n...ETC.\n", "19\n"),
            ("5 7 2\nGET..ET\n..T....\n.TEST..\n.E.T.ET\n...ETC.\n", "21\n"),
            ("5 7 1\nGET..ET\n..T....\n.TEST..\n.E.T.ET\n...ETC.\n", "-1\n"),
            ("6 35 4\nT...TT.....TT...TTT...TTT..TTG.....\n..T..T.TTT.T..T..E..T..E...TTT.TTT.\n\
              .TTT.T.....E.TTTTT.TTT.TTT.TTT.....\n.....T.TT.TT.TTTTT.TTT.TTT.TTTTTTT.\n\
              .TTT.T.TT..T..T..S..T..TTT.TTTTTTT.\n.CTT.E.TTT.TT...TTT...TT.....E.....\n",
             "94\n"),
        ];
        static D: &[(&str, &str)] = &[];
        static EXPECTED: Expected = &[
            ("A", "/contests/arc019/tasks/arc019_1", 2000, A),
            ("B", "/contests/arc019/tasks/arc019_2", 2000, B),
            ("C", "/contests/arc019/tasks/arc019_3", 2000, C),
            ("D", "/contests/arc019/tasks/arc019_4", 2000, D),
        ];
        let _ = env_logger::try_init();
        test_sample_extraction("arc019", EXPECTED);
    }

    #[test]
    fn it_extracts_timelimits_and_sample_cases_from_arc058() {
        static C: &[(&str, &str)] = &[
            ("1000 8\n1 3 4 5 6 7 8 9\n", "2000\n"),
            ("9999 1\n0\n", "9999\n"),
        ];
        static D: &[(&str, &str)] = &[
            ("2 3 1 1\n", "2\n"),
            ("10 7 3 4\n", "3570\n"),
            ("100000 100000 99999 99999\n", "1\n"),
            ("100000 100000 44444 55555\n", "738162020\n"),
        ];
        static E: &[(&str, &str)] = &[
            ("3 5 7 5\n", "1\n"),
            ("4 5 7 5\n", "34\n"),
            ("37 4 2 3\n", "863912418\n"),
            ("40 5 7 5\n", "562805100\n"),
        ];
        static F: &[(&str, &str)] = &[
            ("3 7\nat\ncoder\ncodar\n", "atcodar\n"),
            ("3 7\ncoder\ncodar\nat\n", "codarat\n"),
            ("4 13\nkyuri\nnamida\nzzzzzzz\naaaaaa\n", "namidazzzzzzz\n"),
        ];
        static EXPECTED: Expected = &[
            ("C", "/contests/arc058/tasks/arc058_a", 2000, C),
            ("D", "/contests/arc058/tasks/arc058_b", 2000, D),
            ("E", "/contests/arc058/tasks/arc058_c", 4000, E),
            ("F", "/contests/arc058/tasks/arc058_d", 5000, F),
        ];
        let _ = env_logger::try_init();
        test_sample_extraction("arc058", EXPECTED);
    }

    #[test]
    fn it_extracts_timelimits_and_sample_cases_from_abc041() {
        static A: &[(&str, &str)] = &[
            ("atcoder\n3\n", "c\n"),
            ("beginner\n1\n", "b\n"),
            ("contest\n7\n", "t\n"),
            ("z\n1\n", "z\n"),
        ];
        static B: &[(&str, &str)] = &[
            ("2 3 4\n", "24\n"),
            ("10000 1000 100\n", "1000000000\n"),
            ("100000 1 100000\n", "999999937\n"),
            ("1000000000 1000000000 1000000000\n", "999999664\n"),
        ];
        static C: &[(&str, &str)] = &[
            ("3\n140 180 160\n", "2\n3\n1\n"),
            ("2\n1000000000 1\n", "1\n2\n"),
            ("8\n3 1 4 15 9 2 6 5\n", "4\n5\n7\n8\n3\n1\n6\n2\n"),
        ];
        static D: &[(&str, &str)] = &[
            ("3 2\n2 1\n2 3\n", "2\n"),
            ("5 5\n1 2\n2 3\n3 5\n1 4\n4 5\n", "3\n"),
            ("16 1\n1 2\n", "10461394944000\n"),
        ];
        static EXPECTED: Expected = &[
            ("A", "/contests/abc041/tasks/abc041_a", 2000, A),
            ("B", "/contests/abc041/tasks/abc041_b", 2000, B),
            ("C", "/contests/abc041/tasks/abc041_c", 2000, C),
            ("D", "/contests/abc041/tasks/abc041_d", 3000, D),
        ];
        let _ = env_logger::try_init();
        test_sample_extraction("abc041", EXPECTED);
    }

    #[rustfmt::skip]
    #[test]
    fn it_extracts_timelimits_and_sample_cases_from_chokudai_s001() {
        static A: &[(&str, &str)] = &[
            ("5\n3 1 5 4 2\n", "5\n"),
            ("6\n1 2 3 4 5 6\n", "6\n"),
            ("7\n7 6 5 4 3 2 1\n", "7\n"),
            ("20\n19 11 10 7 8 9 17 18 20 4 3 15 16 1 5 14 6 2 13 12\n", "20\n"),
        ];
        static B: &[(&str, &str)] = &[
            ("5\n3 1 5 4 2\n", "15\n"),
            ("6\n1 2 3 4 5 6\n", "21\n"),
            ("7\n7 6 5 4 3 2 1\n", "28\n"),
            ("20\n19 11 10 7 8 9 17 18 20 4 3 15 16 1 5 14 6 2 13 12\n", "210\n"),
        ];
        static C: &[(&str, &str)] = &[
            ("5\n3 1 5 4 2\n", "3,1,5,4,2\n"),
            ("6\n1 2 3 4 5 6\n", "1,2,3,4,5,6\n"),
            ("7\n7 6 5 4 3 2 1\n", "7,6,5,4,3,2,1\n"),
            ("20\n19 11 10 7 8 9 17 18 20 4 3 15 16 1 5 14 6 2 13 12\n",
             "19,11,10,7,8,9,17,18,20,4,3,15,16,1,5,14,6,2,13,12\n"),
        ];
        static D: &[(&str, &str)] = &[
            ("5\n3 1 5 4 2\n", "1 2 3 4 5\n"),
            ("6\n1 2 3 4 5 6\n", "1 2 3 4 5 6\n"),
            ("7\n7 6 5 4 3 2 1\n", "1 2 3 4 5 6 7\n"),
            ("20\n19 11 10 7 8 9 17 18 20 4 3 15 16 1 5 14 6 2 13 12\n",
             "1 2 3 4 5 6 7 8 9 10 11 12 13 14 15 16 17 18 19 20\n"),
        ];
        static E: &[(&str, &str)] = &[
            ("5\n3 1 5 4 2\n", "2\n"),
            ("6\n1 2 3 4 5 6\n", "1\n"),
            ("7\n7 6 5 4 3 2 1\n", "7\n"),
            ("20\n19 11 10 7 8 9 17 18 20 4 3 15 16 1 5 14 6 2 13 12\n", "14\n"),
        ];
        static F: &[(&str, &str)] = &[
            ("5\n3 1 5 4 2\n", "2\n"),
            ("6\n1 2 3 4 5 6\n", "6\n"),
            ("7\n7 6 5 4 3 2 1\n", "1\n"),
            ("20\n19 11 10 7 8 9 17 18 20 4 3 15 16 1 5 14 6 2 13 12\n", "2\n"),
        ];
        static G: &[(&str, &str)] = &[
            ("5\n3 1 5 4 2\n", "31542\n"),
            ("6\n1 2 3 4 5 6\n", "123456\n"),
            ("7\n7 6 5 4 3 2 1\n", "7654321\n"),
            ("20\n19 11 10 7 8 9 17 18 20 4 3 15 16 1 5 14 6 2 13 12\n", "370453866\n"),
        ];
        static H: &[(&str, &str)] = &[
            ("5\n3 1 5 4 2\n", "2\n"),
            ("6\n1 2 3 4 5 6\n", "6\n"),
            ("7\n7 6 5 4 3 2 1\n", "1\n"),
            ("20\n19 11 10 7 8 9 17 18 20 4 3 15 16 1 5 14 6 2 13 12\n", "6\n"),
        ];
        static I: &[(&str, &str)] = &[
            ("5\n3 1 5 4 2\n", "1\n"),
            ("6\n1 2 3 4 5 6\n", "2\n"),
            ("7\n7 6 5 4 3 2 1\n", "2\n"),
            ("20\n19 11 10 7 8 9 17 18 20 4 3 15 16 1 5 14 6 2 13 12\n", "3\n"),
        ];
        static J: &[(&str, &str)] = &[
            ("5\n3 1 5 4 2\n", "5\n"),
            ("6\n1 2 3 4 5 6\n", "0\n"),
            ("7\n7 6 5 4 3 2 1\n", "21\n"),
            ("20\n19 11 10 7 8 9 17 18 20 4 3 15 16 1 5 14 6 2 13 12\n", "114\n"),
        ];
        static K: &[(&str, &str)] = &[
            ("5\n3 1 5 4 2\n", "54\n"),
            ("6\n1 2 3 4 5 6\n", "1\n"),
            ("7\n7 6 5 4 3 2 1\n", "5040\n"),
            ("20\n19 11 10 7 8 9 17 18 20 4 3 15 16 1 5 14 6 2 13 12\n", "542869439\n"),
        ];
        static L: &[(&str, &str)] = &[
            ("5\n3 1 5 4 2\n", "YES\n"),
            ("6\n1 2 3 4 5 6\n", "YES\n"),
            ("7\n7 6 5 4 3 2 1\n", "YES\n"),
            ("20\n19 11 10 7 8 9 17 18 20 4 3 15 16 1 5 14 6 2 13 12\n", "YES\n"),
        ];
        static EXPECTED: Expected = &[
            ("A", "/contests/chokudai_s001/tasks/chokudai_S001_a", 2000, A),
            ("B", "/contests/chokudai_s001/tasks/chokudai_S001_b", 2000, B),
            ("C", "/contests/chokudai_s001/tasks/chokudai_S001_c", 2000, C),
            ("D", "/contests/chokudai_s001/tasks/chokudai_S001_d", 2000, D),
            ("E", "/contests/chokudai_s001/tasks/chokudai_S001_e", 2000, E),
            ("F", "/contests/chokudai_s001/tasks/chokudai_S001_f", 2000, F),
            ("G", "/contests/chokudai_s001/tasks/chokudai_S001_g", 2000, G),
            ("H", "/contests/chokudai_s001/tasks/chokudai_S001_h", 2000, H),
            ("I", "/contests/chokudai_s001/tasks/chokudai_S001_i", 2000, I),
            ("J", "/contests/chokudai_s001/tasks/chokudai_S001_j", 2000, J),
            ("K", "/contests/chokudai_s001/tasks/chokudai_S001_k", 2000, K),
            ("L", "/contests/chokudai_s001/tasks/chokudai_S001_l", 2000, L),
        ];
        let _ = env_logger::try_init();
        test_sample_extraction("chokudai_s001", EXPECTED);
    }

    type Expected = &'static [(
        &'static str,
        &'static str,
        u64,
        &'static [(&'static str, &'static str)],
    )];

    fn test_sample_extraction(contest: &str, expected: Expected) {
        let mut atcoder = start().unwrap();
        let contest = AtcoderContest::new(contest);
        let page = atcoder.fetch_tasks_page(&contest).unwrap();
        let urls_and_names = page.extract_task_urls_with_names().unwrap();
        for (
            (actual_name, actual_url),
            (expected_name, expected_url, expected_timelimit, expected_samples),
        ) in urls_and_names.iter().zip(expected.iter())
        {
            assert_eq!(expected_name, actual_name);
            assert_eq!(expected_url, actual_url);
            let problem_page = atcoder.get(&actual_url).recv_html().unwrap();
            let expected_timelimit = Duration::from_millis(*expected_timelimit);
            let expected_suite = TestSuite::from(
                SimpleSuite::new(expected_timelimit).cases(expected_samples.iter().cloned()),
            );
            let actual_suite = problem_page.extract_as_suite().unwrap();
            assert_eq!(expected_suite, actual_suite);
        }
    }

    #[test]
    fn it_extracts_a_submitted_source_code() {
        static URL: &str = "/contests/utpc2011/submissions/2067";
        static EXPECTED_CODE: &str =
            "import java.util.*;\n\
             import java.math.*;\n\
             import static java.lang.Math.*;\n\
             import static java.util.Arrays.*;\n\
             import static java.util.Collections.*;\n\
             public class Main{\n\
             \tpublic static void main(String[] args) {\n\
             \t\tnew Main().run();\n\
             \t}\n\
             \tScanner sc = new Scanner(System.in);\n\
             \tvoid run() {\n\
             \t\tint m=sc.nextInt(),n=sc.nextInt();\n\
             \t\tint[] as=new int[m];\n\
             \t\tfor(int i=0;i<m;i++)for(int j=0;j<n;j++)as[i]+=sc.nextInt();\n\
             \t\t\tsort(as);\n\
             \t\t\tSystem.out.println(as[m-1]);\n\
             \t}\n\
             }\n\
             ";
        let _ = env_logger::try_init();
        let mut atcoder = start().unwrap();
        let page = atcoder.get(URL).recv_html().unwrap();
        let code = page.extract_submitted_code().unwrap();
        assert_eq!(EXPECTED_CODE, code);
    }

    fn start() -> ServiceResult<Atcoder<impl Term>> {
        let client = service::reqwest_client(Duration::from_secs(60))?;
        let base = UrlBase::new(Host::Domain("beta.atcoder.jp"), true, None);
        let mut term = TermImpl::null();
        let mut runtime = Runtime::new()?;
        let session = HttpSession::try_new(term.stdout(), &mut runtime, client, base, None, true)?;
        Ok(Atcoder {
            term,
            session,
            runtime,
            credentials: UserNameAndPassword::None,
        })
    }
}
