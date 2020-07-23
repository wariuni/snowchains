use anyhow::anyhow;
use cookie_store::CookieStore;
use snowchains_core::web::{
    Atcoder, AtcoderRetrieveFullTestCasesCredentials, AtcoderRetrieveSampleTestCasesCredentials,
    AtcoderRetrieveTestCasesTargets, Cookies, RetrieveFullTestCases, RetrieveTestCases,
    StandardStreamShell,
};
use std::{env, str};
use structopt::StructOpt;
use strum::{EnumString, EnumVariantNames, VariantNames as _};
use termcolor::ColorChoice;

#[derive(StructOpt, Debug)]
struct Opt {
    #[structopt(long)]
    full: bool,

    #[structopt(short, long, value_name("HUMANTIME"))]
    timeout: Option<humantime::Duration>,

    #[structopt(
        long,
        value_name("VIA"),
        default_value("prompt"),
        possible_values(CredentialsVia::VARIANTS)
    )]
    credentials: CredentialsVia,

    #[structopt(short, long, value_name("PROBLEMS"))]
    problems: Option<Vec<String>>,

    contest: String,
}

#[derive(EnumString, EnumVariantNames, Debug)]
#[strum(serialize_all = "kebab-case")]
enum CredentialsVia {
    Prompt,
    Env,
}

fn main() -> anyhow::Result<()> {
    let Opt {
        full,
        timeout,
        credentials,
        problems,
        contest,
    } = Opt::from_args();

    let mut cookies_jsonl = vec![];

    let outcome = Atcoder::exec(RetrieveTestCases {
        targets: AtcoderRetrieveTestCasesTargets {
            contest,
            problems: problems.map(|ps| ps.into_iter().collect()),
        },
        credentials: AtcoderRetrieveSampleTestCasesCredentials {
            username_and_password: &mut || {
                let username_and_password = match credentials {
                    CredentialsVia::Prompt => (
                        rprompt::prompt_reply_stderr("Username: ")?,
                        rpassword::read_password_from_tty(Some("Password: "))?,
                    ),
                    CredentialsVia::Env => {
                        (env::var("ATCODER_USERNAME")?, env::var("ATCODER_PASSWORD")?)
                    }
                };
                Ok(username_and_password)
            },
        },
        full: if full {
            Some(RetrieveFullTestCases {
                credentials: AtcoderRetrieveFullTestCasesCredentials {
                    dropbox_access_token: match credentials {
                        CredentialsVia::Prompt => {
                            rpassword::read_password_from_tty(Some("Dropbox access token: "))?
                        }
                        CredentialsVia::Env => env::var("DROPBOX_ACCESS_TOKEN")?,
                    },
                },
            })
        } else {
            None
        },
        cookies: Cookies {
            cookie_store: CookieStore::default(),
            on_update_cookie_store: &mut |cookie_store| -> _ {
                cookies_jsonl.clear();
                cookie_store
                    .save_json(&mut cookies_jsonl)
                    .map_err(|e| anyhow!("{}", e))?;
                Ok(())
            },
        },
        timeout: timeout.map(Into::into),
        shell: StandardStreamShell::new(if atty::is(atty::Stream::Stderr) {
            ColorChoice::Auto
        } else {
            ColorChoice::Never
        }),
    })?;

    dbg!(outcome);
    eprintln!("\n{}", str::from_utf8(&cookies_jsonl)?);

    Ok(())
}
