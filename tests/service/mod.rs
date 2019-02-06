#![allow(dead_code)]

use snowchains::app::{App, Opt};
use snowchains::path::{AbsPath, AbsPathBuf};
use snowchains::service::ServiceName;
use snowchains::terminal::{AnsiColorChoice, TermImpl};

use failure::Fallible;
use serde_derive::Deserialize;
use serde_json::json;
use tempdir::TempDir;

use std::fs::File;
use std::panic::UnwindSafe;
use std::{env, panic};

pub fn test_in_tempdir<E: Into<failure::Error>>(
    tempdir_prefix: &str,
    stdin: &str,
    f: impl FnOnce(App<TermImpl<&[u8], Vec<u8>, Vec<u8>>>) -> Result<(), E> + UnwindSafe,
) -> Fallible<()> {
    let tempdir = TempDir::new(tempdir_prefix)?;
    let tempdir_path = tempdir.path().to_owned();
    let result = panic::catch_unwind(move || -> Fallible<()> {
        std::fs::write(
            tempdir_path.join("snowchains.yaml"),
            include_bytes!("../snowchains.yaml").as_ref(),
        )?;
        std::fs::create_dir(tempdir_path.join("local"))?;
        serde_json::to_writer(
            File::create(tempdir_path.join("local").join("dropbox.json"))?,
            &json!({ "access_token": env_var("DROPBOX_ACCESS_TOKEN")? }),
        )?;
        let app = App {
            working_dir: AbsPathBuf::try_new(&tempdir_path).unwrap(),
            login_retries: Some(0),
            term: TermImpl::new(stdin.as_bytes(), vec![], vec![]),
        };
        f(app).map_err(Into::into)
    });
    tempdir.close()?;
    match result {
        Err(panic) => panic::resume_unwind(panic),
        Ok(result) => result,
    }
}

pub fn env_var(name: &'static str) -> Fallible<String> {
    env::var(name).map_err(|err| failure::err_msg(format!("Failed to read {:?}: {}", name, err)))
}

pub fn login(
    mut app: App<TermImpl<&[u8], Vec<u8>, Vec<u8>>>,
    service: ServiceName,
) -> snowchains::Result<()> {
    app.run(Opt::Login {
        color_choice: AnsiColorChoice::Never,
        service,
    })
}

pub fn download(
    mut app: App<TermImpl<&[u8], Vec<u8>, Vec<u8>>>,
    service: ServiceName,
    contest: &str,
    problems: &[&str],
) -> snowchains::Result<()> {
    app.run(Opt::Download {
        open: false,
        only_scraped: false,
        service: Some(service),
        contest: Some(contest.to_owned()),
        problems: problems.iter().map(|&s| s.to_owned()).collect(),
        color_choice: AnsiColorChoice::Never,
    })
}

pub fn confirm_num_cases(
    wd: &AbsPath,
    service: ServiceName,
    contest: &str,
    pairs: &[(&str, usize)],
) -> Fallible<()> {
    #[derive(Deserialize)]
    struct BatchSuite {
        cases: Vec<serde_yaml::Value>,
    }

    for &(problem, expected_num_cases) in pairs {
        let path = wd
            .join(<&str>::from(service))
            .join(contest)
            .join("tests")
            .join(format!("{}.yaml", problem));
        let file = File::open(&path)?;
        let suite = serde_yaml::from_reader::<_, BatchSuite>(file)?;
        assert_eq!(expected_num_cases, suite.cases.len());
    }
    Ok(())
}