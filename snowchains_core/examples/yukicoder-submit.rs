use anyhow::Context as _;
use snowchains_core::web::{
    StandardStreamShell, Submit, Yukicoder, YukicoderSubmitCredentials, YukicoderSubmitTarget,
};
use std::{env, fs, path::PathBuf, str};
use structopt::StructOpt;
use strum::{EnumString, EnumVariantNames, VariantNames as _};
use termcolor::ColorChoice;

#[derive(StructOpt, Debug)]
struct Opt {
    #[structopt(short, long, value_name("HUMANTIME"))]
    timeout: Option<humantime::Duration>,

    #[structopt(
        long,
        value_name("VIA"),
        default_value("prompt"),
        possible_values(CredentialsVia::VARIANTS)
    )]
    credentials: CredentialsVia,

    #[structopt(long, value_name("CONTEST_ID"))]
    contest: Option<u64>,

    problem_no_or_index: String,

    language_id: String,

    file: PathBuf,
}

#[derive(EnumString, EnumVariantNames, Debug)]
#[strum(serialize_all = "kebab-case")]
enum CredentialsVia {
    Prompt,
    Env,
}

fn main() -> anyhow::Result<()> {
    let Opt {
        timeout,
        credentials,
        contest,
        problem_no_or_index,
        language_id,
        file,
    } = Opt::from_args();

    let api_key = match credentials {
        CredentialsVia::Prompt => rpassword::read_password_from_tty(Some("yukicoder API Key: "))?,
        CredentialsVia::Env => env::var("YUKICODER_API_KEY")?,
    };

    let outcome = Yukicoder::exec(Submit {
        target: if let Some(contest) = contest {
            YukicoderSubmitTarget::Contest(contest.to_string(), problem_no_or_index)
        } else {
            YukicoderSubmitTarget::ProblemNo(problem_no_or_index)
        },
        credentials: YukicoderSubmitCredentials { api_key },
        language_id,
        code: fs::read_to_string(&file)
            .with_context(|| format!("Failed to read {}", file.display()))?,
        watch_submission: false,
        cookie_storage: (),
        timeout: timeout.map(Into::into),
        shell: StandardStreamShell::new(if atty::is(atty::Stream::Stderr) {
            ColorChoice::Auto
        } else {
            ColorChoice::Never
        }),
    })?;

    dbg!(outcome);

    Ok(())
}