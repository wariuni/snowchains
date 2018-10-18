use config::{self, Config};
use errors::ExpandTemplateResult;
use judging::{self, JudgeParams};
use path::AbsPathBuf;
use service::{
    atcoder, hackerrank, yukicoder, Credentials, DownloadProp, RestoreProp, ServiceName,
    SessionProp, SubmitProp,
};
use terminal::{AnsiColorChoice, Term};
use testsuite::{self, SerializableExtension};

use once_cell::sync::Lazy;
use regex::Regex;
use structopt::clap::Arg;

use std;
use std::borrow::Cow;
use std::io::Write as _Write;
use std::num::NonZeroUsize;
use std::path::PathBuf;
use std::time::Duration;

#[derive(Debug, StructOpt)]
#[structopt(
    usage = "snowchains <i|init> [OPTIONS] [directory]\
             \n    snowchains <w|switch> [OPTIONS]\
             \n    snowchains <l|login> [OPTIONS] <service>\
             \n    snowchains <p|participate> [OPTIONS] <service> <contest>\
             \n    snowchains <d|download> [FLAGS] [OPTIONS]\
             \n    snowchains <r|restore> [OPTIONS]\
             \n    snowchains <j|judge> [FLAGS] [OPTIONS] <problem>\
             \n    snowchains <s|submit> [FLAGS] [OPTIONS] <problem>\
             \n    snowchains show num-cases [OPTIONS] <problem> <extension>\
             \n    snowchains show timelimit-millis [OPTIONS] <problem> <nth>\
             \n    snowchains show in [OPTIONS] <problem> <nth>\
             \n    snowchains show accepts [OPTIONS] <problem> <nth>\
             \n    snowchains modify timelimit [OPTIONS] <problem> <nth> [timelimit]\
             \n    snowchains modify append [OPTIONS] <problem> <extensioon> <input> [output]"
)]
pub enum Opt {
    #[structopt(
        about = "Creates a \"snowchains.yaml\"",
        name = "init",
        usage = "snowchains <i|init> [OPTIONS] [directory]",
        raw(alias = "\"i\"", display_order = "1"),
    )]
    Init {
        #[structopt(raw(color_choice = "1"))]
        _color_choice: AnsiColorChoice,
        #[structopt(
            help = "Directory to create a \"snowchains.yaml\"",
            default_value = ".",
            parse(from_os_str),
        )]
        directory: PathBuf,
    },

    #[structopt(
        about = "Changes attribute values of a \"snowchains.yaml\"",
        name = "switch",
        usage = "snowchains <w|switch> [OPTIONS]",
        raw(alias = "\"w\"", display_order = "2"),
    )]
    Switch {
        #[structopt(
            raw(service = r#"&["atcoder", "hackerrank", "yukicoder", "other"], Kind::Option(1)"#),
        )]
        service: Option<ServiceName>,
        #[structopt(raw(contest = "Kind::Option(2)"))]
        contest: Option<String>,
        #[structopt(raw(language = "3"))]
        language: Option<String>,
        #[structopt(raw(color_choice = "4"))]
        color_choice: AnsiColorChoice,
    },

    #[structopt(
        about = "Logges in to a service",
        name = "login",
        usage = "snowchains <l|login> [OPTIONS] <service>",
        raw(alias = "\"l\"", display_order = "3"),
    )]
    Login {
        #[structopt(raw(color_choice = "1"))]
        color_choice: AnsiColorChoice,
        #[structopt(raw(service = r#"&["atcoder", "hackerrank", "yukicoder"], Kind::Arg"#))]
        service: ServiceName,
    },

    #[structopt(
        about = "Participates in a contest",
        name = "participate",
        usage = "snowchains <p|participate> [OPTIONS] <service> <contest>",
        raw(alias = "\"p\"", display_order = "4"),
    )]
    Participate {
        #[structopt(raw(color_choice = "1"))]
        color_choice: AnsiColorChoice,
        #[structopt(raw(service = r#"&["atcoder"], Kind::Arg"#))]
        service: ServiceName,
        #[structopt(raw(contest = "Kind::Arg"))]
        contest: String,
    },

    #[structopt(
        about = "Downloads test cases",
        name = "download",
        usage = "snowchains <d|download> [FLAGS] [OPTIONS]",
        raw(alias = "\"d\"", display_order = "5"),
    )]
    Download {
        #[structopt(raw(open_browser = "1"))]
        open_browser: bool,
        #[structopt(raw(service = r#"&["atcoder", "hackerrank", "yukicoder"], Kind::Option(1)"#))]
        service: Option<ServiceName>,
        #[structopt(raw(contest = "Kind::Option(2)"))]
        contest: Option<String>,
        #[structopt(raw(problems = "3"))]
        problems: Vec<String>,
        #[structopt(raw(color_choice = "4"))]
        color_choice: AnsiColorChoice,
    },

    #[structopt(
        about = "Downloads the source codes you've submitted",
        name = "restore",
        usage = "snowchains <r|restore> [OPTIONS]",
        raw(alias = "\"r\"", display_order = "6"),
    )]
    Restore {
        #[structopt(raw(service = "&[\"atcoder\"], Kind::Option(1)"))]
        service: Option<ServiceName>,
        #[structopt(raw(contest = "Kind::Option(2)"))]
        contest: Option<String>,
        #[structopt(raw(problems = "3"))]
        problems: Vec<String>,
        #[structopt(raw(color_choice = "4"))]
        color_choice: AnsiColorChoice,
    },

    #[structopt(
        about = "Tests a binary or script",
        name = "judge",
        usage = "snowchains <j|judge> [FLAGS] [OPTIONS] <problem>",
        raw(alias = "\"j\"", display_order = "7"),
    )]
    Judge {
        #[structopt(raw(force_compile = "1"))]
        force_compile: bool,
        #[structopt(raw(service = "SERVICE_VALUES, Kind::Option(1)"))]
        service: Option<ServiceName>,
        #[structopt(raw(contest = "Kind::Option(2)"))]
        contest: Option<String>,
        #[structopt(raw(language = "3"))]
        language: Option<String>,
        #[structopt(
            parse(try_from_str = "parse_non_zero_usize"),
            raw(jobs = "4"),
        )]
        jobs: Option<NonZeroUsize>,
        #[structopt(raw(color_choice = "4"))]
        color_choice: AnsiColorChoice,
        #[structopt(raw(problem = ""))]
        problem: String,
    },

    #[structopt(
        about = "Submits a source code",
        name = "submit",
        usage = "snowchains <s|submit> [FLAGS] [OPTIONS] <problem>",
        raw(alias = "\"s\"", display_order = "8"),
    )]
    Submit {
        #[structopt(raw(open_browser = "1"))]
        open_browser: bool,
        #[structopt(raw(force_compile = "2", conflicts_with = "\"skip_judging\""))]
        force_compile: bool,
        #[structopt(
            long = "skip-judging",
            help = "Skips judging",
            raw(conflicts_with = "\"force_compile\"", display_order = "3"),
        )]
        skip_judging: bool,
        #[structopt(
            long = "skip-checking-duplication",
            help = "Submits even if the contest is active and your code is already accepted",
            raw(display_order = "4"),
        )]
        skip_checking_duplication: bool,
        #[structopt(raw(service = "&[\"atcoder\", \"yukicoder\"], Kind::Option(1)"))]
        service: Option<ServiceName>,
        #[structopt(raw(contest = "Kind::Option(2)"))]
        contest: Option<String>,
        #[structopt(raw(language = "3"))]
        language: Option<String>,
        #[structopt(
            parse(try_from_str = "parse_non_zero_usize"),
            raw(jobs = "4"),
        )]
        jobs: Option<NonZeroUsize>,
        #[structopt(raw(color_choice = "5"))]
        color_choice: AnsiColorChoice,
        #[structopt(raw(problem = ""))]
        problem: String,
    },

    #[structopt(
        about = "Prints information",
        name = "show",
        usage = "snowchains show num-cases [OPTIONS] <problem> <extension>\
                 \n    snowchains show timelimit-millis [OPTIONS] <problem> <nth>\
                 \n    snowchains show in [OPTIONS] <problem> <nth>\
                 \n    snowchains show accepts [OPTIONS] <problem> <nth>",
        raw(display_order = "9"),
    )]
    Show(Show),

    #[structopt(
        about = "Modify values",
        name = "modify",
        usage = "snowchains modify timelimit [OPTIONS] <problem> <nth> [timelimit]\
                 \n    snowchains modify append [OPTIONS] <problem> <extensioon> <input> [output]",
        raw(display_order = "10"),
    )]
    Modify(Modify),
}

#[derive(Debug, StructOpt)]
pub enum Show {
    #[structopt(
        about = "Prints number of test cases (without EOL)",
        name = "num-cases",
        raw(display_order = "1"),
    )]
    NumCases {
        #[structopt(raw(service = "SERVICE_VALUES, Kind::Option(1)"))]
        service: Option<ServiceName>,
        #[structopt(raw(contest = "Kind::Option(2)"))]
        contest: Option<String>,
        #[structopt(raw(problem = ""))]
        problem: String,
    },

    #[structopt(
        about = "Prints timelimit (without EOL)",
        name = "timelimit-millis",
        raw(display_order = "2"),
    )]
    TimelimitMillis {
        #[structopt(raw(service = "SERVICE_VALUES, Kind::Option(1)"))]
        service: Option<ServiceName>,
        #[structopt(raw(contest = "Kind::Option(2)"))]
        contest: Option<String>,
        #[structopt(raw(problem = ""))]
        problem: String,
        #[structopt(raw(nth = ""))]
        nth: usize,
    },

    #[structopt(
        about = "Prints \"in\" value",
        name = "in",
        raw(display_order = "3"),
    )]
    In {
        #[structopt(raw(service = "SERVICE_VALUES, Kind::Option(1)"))]
        service: Option<ServiceName>,
        #[structopt(raw(contest = "Kind::Option(2)"))]
        contest: Option<String>,
        #[structopt(raw(problem = ""))]
        problem: String,
        #[structopt(raw(nth = ""))]
        nth: usize,
    },

    #[structopt(
        about = "Tests a value from stdin",
        name = "accepts",
        raw(display_order = "4"),
    )]
    Accepts {
        #[structopt(raw(service = "SERVICE_VALUES, Kind::Option(1)"))]
        service: Option<ServiceName>,
        #[structopt(raw(contest = "Kind::Option(2)"))]
        contest: Option<String>,
        #[structopt(raw(color_choice = "3"))]
        color_choice: AnsiColorChoice,
        #[structopt(raw(problem = ""))]
        problem: String,
        #[structopt(raw(nth = ""))]
        nth: usize,
    },
}

#[derive(Debug, StructOpt)]
pub enum Modify {
    #[structopt(
        about = "Modify a timellimit",
        name = "timelimit",
        raw(display_order = "1"),
    )]
    Timelimit {
        #[structopt(raw(service = "SERVICE_VALUES, Kind::Option(1)"))]
        service: Option<ServiceName>,
        #[structopt(raw(contest = "Kind::Option(2)"))]
        contest: Option<String>,
        #[structopt(raw(color_choice = "3"))]
        color_choice: AnsiColorChoice,
        #[structopt(raw(problem = ""))]
        problem: String,
        #[structopt(raw(extension = r#"&["json", "toml", "yaml", "yml"]"#))]
        extension: SerializableExtension,
        #[structopt(
            help = "Timelimit (\\A[0-9]{1,19}(\\.[0-9]+)?m?s\\z)",
            parse(try_from_str = "parse_timelimit"),
        )]
        timelimit: Option<Duration>,
    },

    #[structopt(
        about = "Appends a test case to a test suite file",
        name = "append",
        raw(display_order = "2"),
    )]
    Append {
        #[structopt(raw(service = "SERVICE_VALUES, Kind::Option(1)"))]
        service: Option<ServiceName>,
        #[structopt(raw(contest = "Kind::Option(2)"))]
        contest: Option<String>,
        #[structopt(raw(color_choice = "3"))]
        color_choice: AnsiColorChoice,
        #[structopt(raw(problem = ""))]
        problem: String,
        #[structopt(raw(extension = r#"&["json", "toml", "yaml", "yml"]"#))]
        extension: SerializableExtension,
        #[structopt(help = "\"input\" value to append")]
        input: String,
        #[structopt(help = "\"expected\" value to append")]
        output: Option<String>,
    },
}

static SERVICE_VALUES: &[&str] = &["atcoder", "hackerrank", "yukicoder", "other"];

enum Kind {
    Option(usize),
    Arg,
}

trait ArgExt {
    fn force_compile(self, order: usize) -> Self;
    fn open_browser(self, order: usize) -> Self;
    fn language(self, order: usize) -> Self;
    fn problems(self, order: usize) -> Self;
    fn jobs(self, order: usize) -> Self;
    fn color_choice(self, order: usize) -> Self;
    fn problem(self) -> Self;
    fn nth(self) -> Self;
    fn extension(self, values: &'static [&'static str]) -> Self;
    fn service(self, values: &'static [&'static str], kind: Kind) -> Self;
    fn contest(self, kind: Kind) -> Self;
}

impl ArgExt for Arg<'static, 'static> {
    fn force_compile(self, order: usize) -> Self {
        self.long("force-compile")
            .help("Force to compile")
            .display_order(order)
    }

    fn open_browser(self, order: usize) -> Self {
        self.short("b")
            .long("--open-browser")
            .help("Opens the pages with your default browser")
            .display_order(order)
    }

    fn language(self, order: usize) -> Self {
        self.short("l")
            .long("language")
            .help("Language name")
            .value_name("STRING")
            .display_order(order)
    }

    fn problems(self, order: usize) -> Self {
        self.short("p")
            .long("problems")
            .help("Problem names")
            .value_name("STRING")
            .display_order(order)
    }

    fn jobs(self, order: usize) -> Self {
        self.short("j")
            .long("jobs")
            .help("Number of jobs")
            .value_name("NUMBER")
            .display_order(order)
    }

    fn color_choice(self, order: usize) -> Self {
        self.short("C")
            .long("color")
            .help("Use colors")
            .required(false)
            .possible_values(&["never", "auto", "always"])
            .value_name("WHEN")
            .default_value("auto")
            .display_order(order)
    }

    fn problem(self) -> Self {
        self.help("Problem name")
    }

    fn nth(self) -> Self {
        self.help("0-based index")
    }

    fn extension(self, values: &'static [&'static str]) -> Self {
        self.help("Extension").possible_values(values)
    }

    fn service(mut self, values: &'static [&'static str], kind: Kind) -> Self {
        self = self.help("Service name").possible_values(values);
        if let Kind::Option(order) = kind {
            self = self
                .short("s")
                .long("service")
                .help("Service name")
                .value_name("SERVICE")
                .display_order(order);
        }
        self
    }

    fn contest(mut self, kind: Kind) -> Self {
        self = self.help("Contest name");
        if let Kind::Option(order) = kind {
            self = self
                .short("c")
                .long("contest")
                .value_name("STRING")
                .display_order(order);
        }
        self
    }
}

fn parse_non_zero_usize(s: &str) -> std::result::Result<NonZeroUsize, String> {
    let n = s.parse::<usize>().map_err(|e| e.to_string())?;
    NonZeroUsize::new(n).ok_or_else(|| "must be non-zero".to_owned())
}

fn parse_timelimit(s: &str) -> std::result::Result<Duration, &'static str> {
    static R: Lazy<Regex> = lazy_regex!(r"\A([0-9]{1,19})(\.[0-9]+)?(m)?s\z");
    let caps = R
        .captures(s)
        .ok_or(r"Input must match \A[0-9]{1,19}(\.[0-9]+)?m?s\z")?;
    Ok({
        let (secs, nanos);
        if caps.get(3).is_none() {
            secs = caps[1].parse().unwrap();
            nanos = caps
                .get(2)
                .map(|m| &m.as_str()[1..])
                .unwrap_or("")
                .as_bytes()
                .iter()
                .scan(1_000_000_000, |k, &c| {
                    *k /= 10;
                    Some(*k * u32::from(c - b'0'))
                }).sum::<u32>();
        } else {
            let millis = caps[1].parse::<u64>().unwrap();
            secs = millis / 1000;
            nanos = 1_000_000 * ((millis % 1000) as u32) + caps
                .get(2)
                .map(|m| &m.as_str()[1..])
                .unwrap_or("")
                .as_bytes()
                .iter()
                .scan(1_000_000, |k, &c| {
                    *k /= 10;
                    Some(*k * u32::from(c - b'0'))
                }).sum::<u32>();
        }
        Duration::new(secs, nanos)
    })
}

pub struct App<T: Term> {
    pub working_dir: AbsPathBuf,
    pub cookies_on_init: Cow<'static, str>,
    pub credentials: Credentials,
    pub term: T,
}

impl<T: Term> App<T> {
    pub fn run(&mut self, opt: Opt) -> ::Result<()> {
        info!("Opt = {:?}", opt);
        let working_dir = self.working_dir.clone();
        let cookies_on_init = self.cookies_on_init.clone();
        match opt {
            Opt::Init { directory, .. } => {
                let wd = working_dir.join(&directory);
                config::init(self.term.stdout(), &wd, &cookies_on_init)?;
            }
            Opt::Switch {
                service,
                contest,
                language,
                color_choice,
            } => {
                let (_, stdout, stderr) = self.term.split_mut();
                config::switch(
                    stdout,
                    stderr,
                    color_choice,
                    &working_dir,
                    service,
                    contest,
                    language,
                )?;
            }
            Opt::Login {
                color_choice,
                service,
            } => {
                let config = Config::load(service, None, &working_dir)?;
                self.term.setup(color_choice, config.console());
                let sess_prop = self.sess_prop(&config)?;
                match service {
                    ServiceName::Atcoder => atcoder::login(sess_prop),
                    ServiceName::Hackerrank => hackerrank::login(sess_prop),
                    ServiceName::Yukicoder => yukicoder::login(sess_prop),
                    ServiceName::Other => unreachable!(),
                }?;
            }
            Opt::Participate {
                color_choice,
                service,
                contest,
            } => {
                let config = Config::load(service, contest.clone(), &working_dir)?;
                self.term.setup(color_choice, config.console());
                let sess_prop = self.sess_prop(&config)?;
                match service {
                    ServiceName::Atcoder => atcoder::participate(&contest, sess_prop),
                    _ => unreachable!(),
                }?;
            }
            Opt::Download {
                open_browser,
                service,
                contest,
                problems,
                color_choice,
            } => {
                let config = Config::load(service, contest, &working_dir)?;
                self.term.setup(color_choice, config.console());
                let sess_prop = self.sess_prop(&config)?;
                let download_prop = DownloadProp::try_new(&config, open_browser, problems)?;
                match config.service() {
                    ServiceName::Atcoder => atcoder::download(sess_prop, download_prop),
                    ServiceName::Hackerrank => hackerrank::download(sess_prop, download_prop),
                    ServiceName::Yukicoder => yukicoder::download(sess_prop, download_prop),
                    ServiceName::Other => return Err(::Error::Unimplemented),
                }?;
            }
            Opt::Restore {
                service,
                contest,
                problems,
                color_choice,
            } => {
                let config = Config::load(service, contest, &working_dir)?;
                self.term.setup(color_choice, config.console());
                let sess_prop = self.sess_prop(&config)?;
                let restore_prop = RestoreProp::try_new(&config, problems)?;
                match config.service() {
                    ServiceName::Atcoder => atcoder::restore(sess_prop, restore_prop)?,
                    _ => return Err(::Error::Unimplemented),
                };
            }
            Opt::Judge {
                force_compile,
                service,
                contest,
                language,
                jobs,
                color_choice,
                problem,
            } => {
                let config = Config::load(service, contest, &working_dir)?;
                self.term.setup(color_choice, config.console());
                let (_, stdout, stderr) = self.term.split_mut();
                judging::judge(JudgeParams {
                    stdout,
                    stderr,
                    config: &config,
                    problem: &problem,
                    language: language.as_ref().map(String::as_ref),
                    force_compile,
                    jobs,
                })?;
            }
            Opt::Submit {
                open_browser,
                force_compile,
                skip_judging,
                skip_checking_duplication,
                language,
                service,
                contest,
                jobs,
                color_choice,
                problem,
            } => {
                let language = language.as_ref().map(String::as_str);
                let config = Config::load(service, contest, &working_dir)?;
                self.term.setup(color_choice, config.console());
                if !skip_judging {
                    let (_, mut stdout, stderr) = self.term.split_mut();
                    judging::judge(JudgeParams {
                        stdout: &mut stdout,
                        stderr,
                        config: &config,
                        problem: &problem,
                        language,
                        force_compile,
                        jobs,
                    })?;
                    writeln!(stdout)?;
                }
                let sess_prop = self.sess_prop(&config)?;
                let submit_prop = SubmitProp::try_new(
                    &config,
                    problem.clone(),
                    language,
                    open_browser,
                    skip_checking_duplication,
                )?;
                match config.service() {
                    ServiceName::Atcoder => atcoder::submit(sess_prop, submit_prop)?,
                    ServiceName::Yukicoder => yukicoder::submit(sess_prop, submit_prop)?,
                    _ => return Err(::Error::Unimplemented),
                };
            }
            Opt::Show(Show::NumCases {
                service,
                contest,
                problem,
            }) => {
                let config = Config::load(service, contest, &working_dir)?;
                let num_cases = judging::num_cases(&config, &problem)?;
                write!(self.term.stdout(), "{}", num_cases)?;
                self.term.stdout().flush()?;
            }
            Opt::Show(Show::TimelimitMillis {
                service,
                contest,
                problem,
                nth,
            }) => {
                let config = Config::load(service, contest, &working_dir)?;
                let timelimit = judging::timelimit_millis(&config, &problem, nth)?;
                write!(self.term.stdout(), "{}", timelimit)?;
                self.term.stdout().flush()?;
            }
            Opt::Show(Show::In {
                service,
                contest,
                problem,
                nth,
            }) => {
                let config = Config::load(service, contest, &working_dir)?;
                let input = judging::input(&config, &problem, nth)?;
                write!(self.term.stdout(), "{}", input)?;
                self.term.stdout().flush()?;
            }
            Opt::Show(Show::Accepts {
                service,
                contest,
                color_choice,
                problem,
                nth,
            }) => {
                let config = Config::load(service, contest, &working_dir)?;
                self.term.setup(color_choice, config.console());
                let (stdin, _, stderr) = self.term.split_mut();
                judging::accepts(&config, &problem, nth, stdin, stderr)?;
            }
            Opt::Modify(Modify::Timelimit {
                service,
                contest,
                color_choice,
                problem,
                extension,
                timelimit,
            }) => {
                let config = Config::load(service, contest, &working_dir)?;
                self.term.setup(color_choice, config.console());
                let path = config
                    .download_destinations(Some(extension))
                    .scraping(&problem)?;
                testsuite::modify_timelimit(self.term.stdout(), &problem, &path, timelimit)?;
            }
            Opt::Modify(Modify::Append {
                service,
                contest,
                color_choice,
                problem,
                extension,
                input,
                output,
            }) => {
                let config = Config::load(service, contest, &working_dir)?;
                self.term.setup(color_choice, config.console());
                let path = config
                    .download_destinations(Some(extension))
                    .scraping(&problem)?;
                let output = output.as_ref().map(String::as_str);
                testsuite::append(&problem, &path, &input, output, self.term.stdout())?;
            }
        }
        Ok(())
    }

    fn sess_prop(&mut self, config: &Config) -> ExpandTemplateResult<SessionProp<&mut T>> {
        let cookies_path = config.session_cookies().expand("")?;
        Ok(SessionProp {
            term: &mut self.term,
            domain: config.service().domain(),
            cookies_path,
            timeout: config.session_timeout(),
            credentials: self.credentials.clone(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::parse_timelimit;

    use std::time::Duration;

    #[test]
    fn it_parses_a_timelimit() {
        for (s, t) in &[
            ("0.0s", Duration::new(0, 0)),
            ("0.5678s", Duration::new(0, 567800000)),
            ("1234.0s", Duration::new(1234, 0)),
            ("1234.5678s", Duration::new(1234, 567800000)),
            ("0s", Duration::new(0, 0)),
            ("1234s", Duration::new(1234, 0)),
            ("0.0ms", Duration::new(0, 0)),
            ("0.5678ms", Duration::new(0, 567800)),
            ("1234.0ms", Duration::new(1, 234000000)),
            ("1234.5678ms", Duration::new(1, 234567800)),
            ("0ms", Duration::new(0, 0)),
            ("1234ms", Duration::new(1, 234000000)),
        ] {
            assert_eq!(parse_timelimit(s).unwrap(), *t);
        }
    }
}
