use crate::{shell::Shell, web::LazyLockedFile};
use anyhow::Context as _;
use either::Either;
use heck::KebabCase;
use serde::Serialize;
use snowchains_core::{
    testsuite::{Additional, BatchTestSuite, TestSuite},
    web::{Atcoder, Codeforces, PlatformVariant, RetrieveSampleTestCases, Yukicoder},
};
use std::{
    cell::RefCell,
    io::{BufRead, Write},
    path::PathBuf,
    slice,
};
use structopt::StructOpt;
use strum::VariantNames as _;
use termcolor::{Color, ColorSpec, WriteColor};
use url::Url;

#[derive(StructOpt, Debug)]
pub struct OptRetrieveTestcases {
    /// Prints the output as a JSON value
    #[structopt(long)]
    pub json: bool,

    /// Path to `snowchains.dhall`
    #[structopt(long)]
    pub config: Option<PathBuf>,

    /// Coloring
    #[structopt(
        long,
        possible_values(crate::ColorChoice::VARIANTS),
        default_value("auto")
    )]
    pub color: crate::ColorChoice,

    /// Platform
    #[structopt(
        short,
        long,
        value_name("SERVICE"),
        possible_values(PlatformVariant::KEBAB_CASE_VARIANTS)
    )]
    pub service: Option<PlatformVariant>,

    /// Contest ID
    #[structopt(short, long, value_name("STRING"))]
    pub contest: Option<String>,

    /// Problem indexes (e.g. "a", "b", "c")
    #[structopt(short, long, value_name("STRING"))]
    pub problems: Vec<String>,
}

#[derive(Debug, Serialize)]
struct Outcome {
    problems: Vec<OutcomeProblem>,
}

impl Outcome {
    fn to_json(&self) -> String {
        serde_json::to_string(self).expect("should not fail")
    }
}

#[derive(Debug, Serialize)]
struct OutcomeProblem {
    slug: String,
    url: Url,
    screen_name: String,
    display_name: String,
    test_suite: TestSuite,
}

pub(crate) fn run(
    opt: OptRetrieveTestcases,
    ctx: crate::Context<impl BufRead, impl Write, impl WriteColor>,
) -> anyhow::Result<()> {
    let OptRetrieveTestcases {
        json,
        config,
        color: _,
        service,
        contest,
        problems,
    } = opt;

    let crate::Context {
        cwd,
        mut stdin,
        mut stdout,
        stderr,
        stdin_process_redirection: _,
        stdout_process_redirection: _,
        stderr_process_redirection: _,
        draw_progress: _,
    } = ctx;

    let (detected_target, workspace) = crate::config::detect_target(&cwd, config.as_deref())?;

    let service = service
        .map(Ok)
        .or_else(|| {
            detected_target.service.as_ref().map(|s| {
                s.parse().with_context(|| {
                    "Specified invalid `service` by `detectServiceFromRelativePathSegments`"
                })
            })
        })
        .with_context(|| "`service` is not specified")??;

    let contest = contest.or(detected_target.contest);
    let contest = contest.as_deref();

    let problems = match (&*problems, &detected_target.problem) {
        ([], None) => None,
        ([], Some(problem)) => Some(slice::from_ref(problem)),
        (problems, _) => Some(problems),
    };

    let timeout = Some(crate::web::SESSION_TIMEOUT);

    let cookies_path = crate::web::cookies_path()?;
    let cookies_file = LazyLockedFile::new(&cookies_path);

    let cookie_store = crate::web::load_cookie_store(cookies_file.path())?;
    let on_update_cookie_store =
        |cookie_store: &_| crate::web::save_cookie_store(cookie_store, &cookies_file);

    let stderr = RefCell::new(stderr);
    let shell = Shell::new(&stderr, || unreachable!(), false);

    let username_and_password = || -> _ {
        let mut stderr = stderr.borrow_mut();

        write!(stderr, "Username: ")?;
        stderr.flush()?;
        let username = stdin.read_reply()?;

        write!(stderr, "Password: ")?;
        stderr.flush()?;
        let password = stdin.read_password()?;

        Ok((username, password))
    };

    let outcome = match service {
        PlatformVariant::Atcoder => {
            let targets = {
                let contest = contest.with_context(|| "`contest` is required for AtCoder")?;
                (contest, problems)
            };
            let cookie_store = (cookie_store, on_update_cookie_store);
            let credentials = (username_and_password,);

            Atcoder::exec(RetrieveSampleTestCases {
                targets,
                timeout,
                cookie_store,
                shell,
                credentials,
            })
        }
        PlatformVariant::Codeforces => {
            let targets = {
                let contest = contest
                    .with_context(|| "`contest` is required for Codeforces")?
                    .parse()
                    .with_context(|| "`contest` for Codeforces must be 64-bit unsigned integer")?;
                (contest, problems)
            };
            let cookie_store = (cookie_store, on_update_cookie_store);
            let credentials = (username_and_password,);

            Codeforces::exec(RetrieveSampleTestCases {
                targets,
                timeout,
                cookie_store,
                shell,
                credentials,
            })
        }
        PlatformVariant::Yukicoder => {
            let targets = if let Some(contest) = contest {
                Either::Right((contest, problems))
            } else {
                let nos = problems
                    .with_context(|| "`contest` or `problem`s are required for yukicoder")?
                    .iter()
                    .map(|s| s.parse())
                    .collect::<Result<Vec<_>, _>>()
                    .with_context(|| "`problem`s for yukicoder must be unsigned integer")?;
                Either::Left(nos)
            };
            let targets = match &targets {
                Either::Left(nos) => Either::Left(&**nos),
                Either::Right((contest, problems)) => Either::Right((contest, *problems)),
            };
            let cookie_store = ();
            let credentials = ();

            Yukicoder::exec(RetrieveSampleTestCases {
                targets,
                timeout,
                cookie_store,
                shell,
                credentials,
            })
        }
    }?;

    let mut acc = Outcome { problems: vec![] };

    for snowchains_core::web::RetrieveTestCasesOutcomeProblem {
        slug,
        url,
        screen_name,
        display_name,
        mut test_suite,
        text_files,
    } in outcome.problems
    {
        let path = workspace
            .join(".snowchains")
            .join("tests")
            .join(service.to_kebab_case_str())
            .join(contest.unwrap_or(""))
            .join(slug.to_kebab_case())
            .with_extension("yml");

        let txt_path = |dir_file_name: &str, txt_file_name: &str| -> _ {
            path.with_file_name(slug.to_kebab_case())
                .join(dir_file_name)
                .join(txt_file_name)
                .with_extension("txt")
        };

        for (name, snowchains_core::web::RetrieveTestCasesOutcomeProblemTextFiles { r#in, out }) in
            &text_files
        {
            crate::fs::write(txt_path("in", name), &r#in, true)?;
            if let Some(out) = out {
                crate::fs::write(txt_path("out", name), out, true)?;
            }
        }

        if !text_files.is_empty() {
            if let TestSuite::Batch(BatchTestSuite { cases, extend, .. }) = &mut test_suite {
                cases.clear();

                extend.push(Additional::Text {
                    base: format!("./{}", slug),
                    r#in: "/in/*.txt".to_owned(),
                    out: "/out/*.txt".to_owned(),
                    timelimit: None,
                    r#match: None,
                })
            }
        }

        crate::fs::write(&path, test_suite.to_yaml_pretty(), true)?;

        let mut stderr = stderr.borrow_mut();

        stderr.set_color(ColorSpec::new().set_reset(false).set_bold(true))?;
        write!(stderr, "{}:", slug)?;
        stderr.reset()?;

        write!(stderr, " Saved to ")?;

        stderr.set_color(ColorSpec::new().set_reset(false).set_fg(Some(Color::Cyan)))?;
        if text_files.is_empty() {
            write!(stderr, "{}", path.display())
        } else {
            write!(
                stderr,
                "{}",
                path.with_file_name(format!("{{{slug}.yml, {slug}/}}", slug = slug))
                    .display(),
            )
        }?;
        stderr.reset()?;

        write!(stderr, " (")?;

        let (msg, color) = match &test_suite {
            TestSuite::Batch(BatchTestSuite { cases, .. }) => {
                match cases.len() + text_files.len() {
                    0 => ("no test cases".to_owned(), Color::Yellow),
                    1 => ("1 test case".to_owned(), Color::Green),
                    n => (format!("{} test cases", n), Color::Green),
                }
            }
            TestSuite::Interactive(_) => ("interactive problem".to_owned(), Color::Yellow),
            TestSuite::Unsubmittable => ("unsubmittable problem".to_owned(), Color::Yellow),
        };

        stderr.set_color(ColorSpec::new().set_reset(false).set_fg(Some(color)))?;
        write!(stderr, "{}", msg)?;
        stderr.reset()?;

        writeln!(stderr, ")")?;
        stderr.flush()?;

        acc.problems.push(OutcomeProblem {
            slug,
            url,
            screen_name,
            display_name,
            test_suite,
        });
    }

    if json {
        writeln!(stdout, "{}", acc.to_json())?;
        stdout.flush()?;
    }

    Ok(())
}