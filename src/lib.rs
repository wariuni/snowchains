#![recursion_limit = "1024"]

#[macro_use]
extern crate custom_derive;
#[macro_use]
extern crate error_chain;
#[macro_use]
extern crate lazy_static;
#[macro_use]
extern crate log;
#[macro_use]
extern crate maplit;
#[macro_use]
extern crate newtype_derive;
#[macro_use]
extern crate serde_derive;

extern crate bincode;
extern crate chrono;
extern crate cookie;
extern crate decimal;
extern crate futures;
extern crate httpsession;
extern crate pbr;
extern crate regex;
extern crate robots_txt;
extern crate rpassword;
extern crate rprompt;
extern crate select;
extern crate serde;
extern crate serde_json;
extern crate serde_urlencoded;
extern crate serde_yaml;
extern crate term;
extern crate toml;
extern crate webbrowser;
extern crate zip;

#[cfg(test)]
#[macro_use]
extern crate nickel;

#[cfg(test)]
extern crate env_logger;

#[macro_use]
pub mod macros;

pub mod config;
pub mod errors;
pub mod judging;
pub mod service;
pub mod template;
pub mod terminal;
pub mod testsuite;
pub mod util;

mod command;
mod replacer;

pub use errors::{ErrorKind, Result};

use std::fmt;
use std::str::FromStr;

#[derive(Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ServiceName {
    AtCoder,
    AtCoderBeta,
    HackerRank,
}

impl fmt::Display for ServiceName {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

impl FromStr for ServiceName {
    type Err = String;

    fn from_str(s: &str) -> std::result::Result<Self, String> {
        match s.to_lowercase().as_str() {
            "atcoder" => Ok(ServiceName::AtCoder),
            "atcoderbeta" => Ok(ServiceName::AtCoderBeta),
            "hackerrank" => Ok(ServiceName::HackerRank),
            _ => Err(format!("Unsupported service name: {:?}", s)),
        }
    }
}

impl ServiceName {
    pub fn as_str(self) -> &'static str {
        match self {
            ServiceName::AtCoder => "atcoder",
            ServiceName::AtCoderBeta => "atcoderbeta",
            ServiceName::HackerRank => "hackerrank",
        }
    }
}