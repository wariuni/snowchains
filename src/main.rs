extern crate snowchains;

extern crate env_logger;
extern crate failure;
extern crate structopt;

use snowchains::app::{App, Opt};
use snowchains::path::AbsPathBuf;
use snowchains::service::Credentials;
use snowchains::terminal::{Term, TermImpl, WriteAnsi as _WriteAnsi, WriteSpaces as _WriteSpaces};

use failure::Fail;
use structopt::StructOpt as _StructOpt;

use std::io::{self, Write as _Write};
use std::process;

fn main() -> io::Result<()> {
    env_logger::init();
    let opt = Opt::from_args();
    let (stdin, stdout, stderr) = (io::stdin(), io::stdout(), io::stderr());
    let mut term = TermImpl::new(&stdin, &stdout, &stderr);
    if let Err(err) = run(opt, &mut term) {
        term.stdout().flush()?;
        let mut stderr = term.stderr();
        writeln!(stderr)?;
        for (i, cause) in Fail::iter_chain(&err).enumerate() {
            let head = if i == 0 && err.cause().is_none() {
                "error: "
            } else if i == 0 {
                "    error: "
            } else {
                "caused by: "
            };
            stderr.with_reset(|o| o.fg(1)?.bold()?.write_str(head))?;
            for (i, line) in cause.to_string().lines().enumerate() {
                if i > 0 {
                    stderr.write_spaces(head.len())?;
                }
                writeln!(stderr, "{}", line)?;
            }
        }
        if let Some(backtrace) = err.backtrace() {
            writeln!(stderr, "{:?}", backtrace)?;
        }
        stderr.flush()?;
        process::exit(1)
    } else {
        Ok(())
    }
}

fn run(opt: Opt, term: impl Term) -> snowchains::Result<()> {
    let working_dir = AbsPathBuf::cwd()?;
    App {
        working_dir,
        cookies_on_init: "~/.local/share/snowchains/$service".into(),
        credentials: Credentials::default(),
        term,
    }.run(opt)
}
