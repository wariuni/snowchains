use errors::{
    FileIoError, FileIoErrorKind, SerializeError, SessionError, SessionResult, StartSessionError,
};
use palette::Palette;
use service::USER_AGENT;
use util;

use cookie::{self, CookieJar};
use failure::ResultExt as _ResultExt;
use reqwest::header::{self, Headers, Location, SetCookie};
use reqwest::{self, multipart, Method, Response, StatusCode};
use robots_txt::{Robots, SimpleMatcher};
use select::document::Document;
use serde::Serialize;
use url::{Host, Url};
use {bincode, webbrowser};

use std::borrow::Cow;
use std::collections::HashMap;
use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom, Write as _IoWrite};
use std::path::PathBuf;

pub(super) trait GetPost {
    fn session(&mut self) -> &mut HttpSession;

    fn get(&mut self, url: &str) -> self::Request {
        self.session().get(url)
    }

    fn post(&mut self, url: &str) -> self::Request {
        self.session().post(url)
    }
}

/// A wrapper of `reqwest::Client`.
#[derive(Debug)]
pub(crate) struct HttpSession {
    client: reqwest::Client,
    robots_txts: HashMap<String, String>,
    base: Option<UrlBase>,
    jar: Option<AutosavedCookieJar>,
}

impl HttpSession {
    pub fn new(
        client: reqwest::Client,
        base: impl Into<Option<UrlBase>>,
        cookies_path: impl Into<Option<PathBuf>>,
    ) -> SessionResult<Self> {
        let start = || -> SessionResult<HttpSession> {
            let base = base.into();
            let host = base.as_ref().map(|base| base.host.clone());
            let jar = match cookies_path.into() {
                Some(path) => Some(AutosavedCookieJar::new(path)?),
                None => None,
            };
            let mut sess = Self {
                client,
                robots_txts: hashmap!(),
                base,
                jar,
            };
            if let Some(host) = host {
                let mut res = sess
                    .get("/robots.txt")
                    .acceptable(&[200, 301, 302, 404])
                    .send()?;
                while [301, 302].contains(&res.status().as_u16()) {
                    let location = res
                        .headers()
                        .get::<Location>()
                        .map(|location| (*location).to_owned());
                    if let Some(location) = location {
                        res = sess
                            .get(&location)
                            .acceptable(&[200, 301, 302, 404])
                            .send()?;
                    } else {
                        return Ok(sess);
                    }
                }
                match res.status().as_u16() {
                    200 => {
                        sess.robots_txts.insert(host.to_string(), res.text()?);
                    }
                    404 => (),
                    _ => unreachable!(),
                }
            }
            Ok(sess)
        };
        start().context(StartSessionError).map_err(Into::into)
    }

    /// Whether it has any cookie value.
    pub fn has_cookie(&self) -> bool {
        match self.jar.as_ref() {
            Some(jar) => jar.inner.iter().next().is_some(),
            None => false,
        }
    }

    pub fn cookies_to_header(&self) -> Option<header::Cookie> {
        self.jar.as_ref().map(AutosavedCookieJar::to_header)
    }

    pub fn insert_cookie(&mut self, cookie: cookie::Cookie<'static>) -> SessionResult<()> {
        match self.jar.as_mut() {
            None => Ok(()),
            Some(jar) => jar.insert_cookie(cookie),
        }
    }

    /// Removes all cookies.
    pub fn clear_cookies(&mut self) -> SessionResult<()> {
        if let Some(jar) = self.jar.as_mut() {
            jar.inner = CookieJar::new();
            jar.save()?;
        }
        Ok(())
    }

    /// If `url` starts with '/' and the base host is present, returns
    /// http(s)://<host><url>.
    pub fn resolve_url<'a>(&self, url: &'a str) -> SessionResult<Url> {
        match self.base.as_ref() {
            Some(base) => base.with(url),
            None => Url::parse(url).map_err(|e| SessionError::ParseUrl(url.to_owned(), e)),
        }
    }

    /// Opens `url`, which is relative or absolute, with default browser
    /// printing a message.
    pub fn open_in_browser(&self, url: &str) -> SessionResult<()> {
        let url = self.resolve_url(url)?;
        println!("Opening {} in default browser...", url);
        let status = webbrowser::open(url.as_str())?.status;
        if status.success() {
            Ok(())
        } else {
            Err(SessionError::Webbrowser(status))
        }
    }

    pub fn get(&mut self, url: &str) -> self::Request {
        self.request(url, Method::Get, vec![StatusCode::Ok])
    }

    pub fn post(&mut self, url: &str) -> self::Request {
        self.request(url, Method::Post, vec![StatusCode::Found])
    }

    fn request(&mut self, url: &str, method: Method, acceptable: Vec<StatusCode>) -> self::Request {
        self::Request {
            inner: self.try_request(url, method),
            session: self,
            acceptable,
        }
    }

    fn try_request(&mut self, url: &str, method: Method) -> SessionResult<reqwest::RequestBuilder> {
        let url = self.resolve_url(url)?;
        self.assert_not_forbidden_by_robots_txt(&url)?;
        let mut req = self.client.request(method, url.as_str());
        if let Some(jar) = self.jar.as_ref() {
            req.header(jar.to_header());
        }
        Ok(req)
    }

    fn assert_not_forbidden_by_robots_txt(&self, url: &Url) -> SessionResult<()> {
        if let Some(host) = url.host_str() {
            if let Some(robots_txt) = self.robots_txts.get(host) {
                let robots = Robots::from_str(robots_txt);
                let matcher = SimpleMatcher::new(&robots.choose_section(USER_AGENT).rules);
                if !matcher.check_path(url.path()) {
                    return Err(SessionError::ForbiddenByRobotsTxt);
                }
            }
        }
        Ok(())
    }
}

pub(crate) struct Request<'a> {
    session: &'a mut HttpSession,
    inner: SessionResult<reqwest::RequestBuilder>,
    acceptable: Vec<StatusCode>,
}

impl<'a> Request<'a> {
    pub fn headers(mut self, headers: Headers) -> Self {
        if let Ok(inner) = self.inner.as_mut() {
            inner.headers(headers);
        }
        self
    }

    pub fn acceptable(self, statuses: &[u16]) -> Self {
        let acceptable = statuses
            .iter()
            .map(|&n| StatusCode::try_from(n).unwrap_or_else(|_| StatusCode::Unregistered(n)))
            .collect();
        Self { acceptable, ..self }
    }

    pub fn send(self) -> SessionResult<Response> {
        let req = self.inner?.build()?;
        req.echo_method();
        let res = self.session.client.execute(req).map_err(|err| {
            println!();
            err
        })?;
        res.echo_status(&self.acceptable);
        if let Some(jar) = self.session.jar.as_mut() {
            jar.update(&res)?;
        }
        res.filter_by_status(self.acceptable)
    }

    pub fn send_form(mut self, form: &(impl Serialize + ?Sized)) -> SessionResult<Response> {
        if let Ok(inner) = self.inner.as_mut() {
            inner.form(form);
        }
        self.send()
    }

    pub fn send_json(mut self, json: &(impl Serialize + ?Sized)) -> SessionResult<Response> {
        if let Ok(inner) = self.inner.as_mut() {
            inner.json(json);
        }
        self.send()
    }

    pub fn send_multipart(mut self, multipart: multipart::Form) -> SessionResult<Response> {
        if let Ok(inner) = self.inner.as_mut() {
            inner.multipart(multipart);
        }
        self.send()
    }

    pub fn recv_html(self) -> SessionResult<Document> {
        Ok(Document::from(self.send()?.text()?.as_str()))
    }
}

trait EchoMethod {
    fn echo_method(&self);
}

impl EchoMethod for reqwest::Request {
    fn echo_method(&self) {
        print!(
            "{} {} ... ",
            Palette::Plain.bold().paint(self.method().to_string()),
            Palette::Url.paint(self.url().to_string()),
        );
        io::stdout().flush().unwrap();
    }
}

trait ResponseExt
where
    Self: Sized,
{
    fn echo_status(&self, expected_statuses: &[StatusCode]);
    fn filter_by_status(self, expected: Vec<StatusCode>) -> SessionResult<Self>;
}

impl ResponseExt for Response {
    fn echo_status(&self, expected_statuses: &[StatusCode]) {
        let palette = if expected_statuses.contains(&self.status()) {
            Palette::Success
        } else {
            Palette::Fatal
        };
        println!("{}", palette.bold().paint(self.status().to_string()));
    }

    fn filter_by_status(self, expected: Vec<StatusCode>) -> SessionResult<Self> {
        if expected.is_empty() || expected.contains(&self.status()) {
            Ok(self)
        } else {
            Err(SessionError::UnexpectedStatusCode(expected, self.status()))
        }
    }
}

#[derive(Debug)]
pub(crate) struct UrlBase {
    host: Host<&'static str>,
    https: bool,
    port: Option<u16>,
}

impl UrlBase {
    pub fn new(host: Host<&'static str>, https: bool, port: Option<u16>) -> Self {
        Self { host, https, port }
    }

    fn with(&self, relative_or_absolute_url: &str) -> SessionResult<Url> {
        let mut url = Cow::from(relative_or_absolute_url);
        if url.starts_with('/') {
            url = format!(
                "http{}://{}{}{}",
                if self.https { "s" } else { "" },
                self.host,
                match self.port {
                    Some(port) => format!(":{}", port),
                    None => "".to_owned(),
                },
                url,
            ).into();
        }
        Url::parse(&url).map_err(|e| SessionError::ParseUrl(url.into_owned(), e))
    }
}

#[derive(Debug)]
struct AutosavedCookieJar {
    path: PathBuf,
    file: File,
    inner: CookieJar,
}

impl AutosavedCookieJar {
    fn new(path: impl Into<PathBuf>) -> SessionResult<Self> {
        let path = path.into();
        let exists = path.exists();
        let mut file = util::fs::create_and_lock(&path)?;
        let mut inner = CookieJar::new();
        if exists {
            let mut cookies =
                Vec::with_capacity(file.metadata().map(|m| m.len() as usize + 1).unwrap_or(0));
            file.read_to_end(&mut cookies)
                .map_err(|e| FileIoError::chaining(FileIoErrorKind::Read, &path, e))?;
            if !cookies.is_empty() {
                let cookies = bincode::deserialize::<Vec<String>>(&cookies)
                    .map_err(|e| FileIoError::chaining(FileIoErrorKind::Deserialize, &path, e))?;
                for cookie in cookies {
                    let cookie = cookie::Cookie::parse(cookie.clone()).map_err(|e| {
                        SessionError::ParseCookieFromPath(cookie, path.to_owned(), e)
                    })?;
                    inner.add(cookie);
                }
            }
        } else {
            file.write_all(&bincode::serialize(&Vec::<String>::new()).unwrap())
                .map_err(|e| FileIoError::chaining(FileIoErrorKind::Write, &path, e))?;
        }
        Ok(Self { file, path, inner })
    }

    fn to_header(&self) -> header::Cookie {
        self.inner
            .iter()
            .fold(header::Cookie::new(), |mut header, cookie| {
                header.append(cookie.name().to_owned(), cookie.value().to_owned());
                header
            })
    }

    fn insert_cookie(&mut self, cookie: cookie::Cookie<'static>) -> SessionResult<()> {
        self.inner.add(cookie);
        self.save()
    }

    fn update(&mut self, response: &Response) -> SessionResult<()> {
        if let Some(setcookie) = response.headers().get::<SetCookie>() {
            for cookie in setcookie.iter() {
                let cookie = cookie.to_owned();
                let cookie = cookie::Cookie::parse(cookie.clone()).map_err(|e| {
                    SessionError::ParseCookieFromUrl(cookie, response.url().to_owned(), e)
                })?;
                self.inner.add(cookie);
            }
            self.save()?;
        }
        Ok(())
    }

    fn save(&mut self) -> SessionResult<()> {
        let value = self
            .inner
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>();
        let value = bincode::serialize(&value).map_err(|e| SerializeError::new(&value, e))?;
        self.file
            .seek(SeekFrom::Start(0))
            .and_then(|_| self.file.set_len(0))
            .and_then(|()| self.file.write_all(&value))
            .map_err(|e| FileIoError::chaining(FileIoErrorKind::Write, &self.path, e).into())
    }
}

#[cfg(test)]
mod tests {
    use errors::SessionError;
    use service;
    use service::session::{HttpSession, UrlBase};

    use failure::Fail as _Fail;
    use nickel::{self, Nickel};
    use tempdir::TempDir;
    use url::Host;
    use {env_logger, reqwest};

    use std::net::Ipv4Addr;
    use std::panic;

    #[test]
    #[ignore]
    fn it_works() {
        let _ = env_logger::try_init();
        let server = {
            let mut server = Nickel::new();
            server.utilize(router! {
                get "/" => |_, mut response| {
                    response.headers_mut().set(
                        nickel::hyper::header::SetCookie(vec!["foo=bar".to_owned()]));
                    ""
                }
                get "/robots.txt" => { "User-agent: *\nDisallow: /sensitive" }
            });
            server.listen("127.0.0.1:2000").unwrap()
        };
        let result = panic::catch_unwind(|| {
            let client = service::reqwest_client(None).unwrap();
            let base = UrlBase::new(Host::Ipv4(Ipv4Addr::new(127, 0, 0, 1)), false, Some(2000));
            let mut sess = HttpSession::new(client, Some(base), None).unwrap();
            let res = sess.get("/").send().unwrap();
            assert!(res.headers().get::<reqwest::header::SetCookie>().is_some());
            sess.get("/nonexisting").acceptable(&[404]).send().unwrap();
            sess.get("/nonexisting").acceptable(&[]).send().unwrap();
            match sess.get("/sensitive").send().unwrap_err() {
                SessionError::ForbiddenByRobotsTxt => {}
                err => panic!("{:?}", err),
            }
        });
        server.detach();
        result.unwrap_or_else(|p| panic::resume_unwind(p));
    }

    #[test]
    #[ignore]
    fn it_keeps_a_file_locked_while_alive() {
        let _ = env_logger::try_init();
        let tempdir = TempDir::new("it_keeps_a_file_locked_while_alive").unwrap();
        let path = tempdir.path().join("cookies");
        let client = service::reqwest_client(None).unwrap();
        HttpSession::new(client.clone(), None, path.clone()).unwrap();
        HttpSession::new(client.clone(), None, path.clone()).unwrap();
        let _session = HttpSession::new(client.clone(), None, path.clone()).unwrap();
        let err = HttpSession::new(client, None, path.clone()).unwrap_err();
        if let SessionError::Start(ref ctx) = err {
            if ctx.cause().is_some() {
                return;
            }
        }
        panic!("{:?}", err);
    }
}