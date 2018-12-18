use crate::errors::{ConfigErrorKind, ConfigResult, FileResult};
use crate::judging::command::{CompilationCommand, JudgingCommand, TranspilationCommand};
use crate::path::{AbsPath, AbsPathBuf};
use crate::service::ServiceName;
use crate::template::{
    CompilationCommandRequirements, JudgingCommandRequirements, Template, TemplateBuilder,
    TranspilationCommandRequirements,
};
use crate::terminal::{TermOut, WriteAnsi, WriteSpaces as _WriteSpaces};
use crate::testsuite::{DownloadDestinations, SuiteFileExtension, TestCaseLoader};
use crate::{time, yaml};

use maplit::hashmap;
use serde::ser::SerializeMap as _SerializeMap;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_derive::{Deserialize, Serialize};
use strum::AsStaticRef as _AsStaticRef;

use std::borrow::Cow;
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::ffi::OsString;
use std::io::{self, Write};
use std::num::NonZeroUsize;
use std::str;
use std::time::Duration;

static CONFIG_FILE_NAME: &str = "snowchains.yaml";

/// Creates "snowchains.yaml" in `directory`.
pub(crate) fn init(
    mut stdout: impl Write,
    directory: &AbsPath,
    session_cookies: &str,
    session_dropbox_auth: &str,
    enable_session_dropbox: bool,
) -> FileResult<()> {
    #[cfg(not(windows))]
    static CONSOLE_ALT_WIDTH: &str = "";
    #[cfg(windows)]
    static CONSOLE_ALT_WIDTH: &str = "\n  # alt_width: 100";
    #[cfg(not(windows))]
    static SHELL: &str = "bash: [/bin/bash, -c, $command]";
    #[cfg(windows)]
    static SHELL: &str = "cmd: ['C:\\Windows\\cmd.exe', /C, $command]\n    \
                          ps: [powershell, -Command, $command]";
    #[cfg(not(windows))]
    static EXE: &str = "";
    #[cfg(windows)]
    static EXE: &str = ".exe";
    #[cfg(not(windows))]
    static VENV_PYTHON3: &str = "./venv/bin/python3";
    #[cfg(windows)]
    static VENV_PYTHON3: &str = "./venv/Scripts/python.exe";
    #[cfg(not(windows))]
    static TRANSPILE_JAVA: &str =
        r#"bash: cat "$SRC" | sed -r "s/class\s+$PROBLEM_PASCAL/class Main/g" > "$TRANSPILED""#;
    #[cfg(windows)]
    static TRANSPILE_JAVA: &str =
        "ps: cat ${env:SRC} | \
         % { $_ -replace \"class\\s+${env:PROBLEM_PASCAL}\", \"class Main\" } | \
         sc ${env:TRANSPILED}";
    #[cfg(not(windows))]
    static TRANSPILE_SCALA: &str =
        r#"bash: cat "$SRC" | sed -r "s/object\s+$PROBLEM_PASCAL/object Main/g" > "$TRANSPILED""#;
    #[cfg(windows)]
    static TRANSPILE_SCALA: &str =
        "ps: cat ${env:SRC} | \
         % { $_ -replace \"object\\s+${env:PROBLEM_PASCAL}\", \"object Main\" } | \
         sc ${env:TRANSPILED}";
    #[cfg(not(windows))]
    static CRLF_TO_LF_TRUE: &str = "";
    #[cfg(windows)]
    static CRLF_TO_LF_TRUE: &str = "\n      crlf_to_lf: true";
    #[cfg(not(windows))]
    static CRLF_TO_LF_FALSE: &str = "";
    #[cfg(windows)]
    static CRLF_TO_LF_FALSE: &str = "\n      # crlf_to_lf: false";
    #[cfg(not(windows))]
    static CSHARP: &str = r#"  c#:
    src: cs/{Pascal}/{Pascal}.cs
    compile:
      bin: cs/{Pascal}/bin/Release/{Pascal}.exe
      command: [mcs, -o+, '-r:System.Numerics', '-out:$bin', $src]
      working_directory: cs
    run:
      command: [mono, $bin]
      working_directory: cs
    language_ids:
      # atcoder: 3006        # "C# (Mono x.x.x.x)"
      yukicoder: csharp_mono # "C#(mono) (mono x.x.x.x)""#;
    #[cfg(windows)]
    static CSHARP: &str = r#"  c#:
    src: cs/{Pascal}/{Pascal}.cs
    compile:
      bin: cs/{Pascal}/bin/Release/{Pascal}.exe
      command: [csc, /o+, '/r:System.Numerics', '/out:$bin', $src]
      working_directory: cs
    run:
      command: [$bin]
      working_directory: cs
      crlf_to_lf: true
    language_ids:
      # atcoder: 3006   # "C# (Mono x.x.x.x)"
      yukicoder: csharp # "C# (csc x.x.x.x)""#;
    let config = format!(
        r#"---
service: atcoder
contest: arc100
language: c++

console:
  cjk: false{console_alt_width}

testfile_path: tests/$service/$contest/{{snake}}.$extension

session:
  timeout: 60s
  silent: false
  cookies: {session_cookies}
  {session_dropbox}
  download:
    extension: yaml
    text_file_dir: tests/$service/$contest/{{snake}}

judge:
  jobs: 4
  testfile_extensions: [json, toml, yaml, yml]
  shell:
    {shell}

services:
  atcoder:
    # language: c++
    variables:
      rust_version: 1.15.1
  hackerrank:
    # language: c++
    variables:
      rust_version: 1.29.1
  yukicoder:
    # language: c++
    variables:
      rust_version: 1.30.1
  other:
    # language: c++
    variables:
      rust_version: stable

interactive:
  python3:
    src: testers/py/test-{{kebab}}.py
    run:
      command: [{venv_python3}, $src, $1, $2, $3, $4, $5, $6, $7, $8, $9]
      working_directory: testers/py{crlf_to_lf_true}
  haskell:
    src: testers/hs/app/Test{{Pascal}}.hs
    compile:
      bin: testers/hs/target/Test{{Pascal}}
      command: [stack, ghc, --, -O2, -o, $bin, $src]
      working_directory: testers/hs
    run:
      command: [$bin, $1, $2, $3, $4, $5, $6, $7, $8, $9]
      working_directory: testers/hs{crlf_to_lf_false}

languages:
  c++:
    src: cpp/{{kebab}}.cpp     # source file to test and to submit
    compile:                 # optional
      bin: cpp/build/{{kebab}}{exe}
      command: [g++, -std=c++14, -Wall, -Wextra, -g, -fsanitize=undefined, -D_GLIBCXX_DEBUG, -o, $bin, $src]
      working_directory: cpp # default: "."
    run:
      command: [$bin]
      working_directory: cpp # default: "."{crlf_to_lf_true}
    language_ids:            # optional
      atcoder: 3003          # "C++14 (GCC x.x.x)"
      yukicoder: cpp14       # "C++14 (gcc x.x.x)"
  rust:
    src: rs/src/bin/{{kebab}}.rs
    compile:
      bin: rs/target/manually/{{kebab}}{exe}
      command: [rustc, +$rust_version, -o, $bin, $src]
      working_directory: rs
    run:
      command: [$bin]
      working_directory: rs{crlf_to_lf_false}
    # language_ids:
    #   atcoder: 3504   # "Rust (x.x.x)"
    #   yukicoder: rust # "Rust (x.x.x)"
  go:
    src: go/{{kebab}}.go
    compile:
      bin: go/{{kebab}}{exe}
      command: [go, build, -o, $bin, $src]
      working_directory: go
    run:
      command: [$bin]
      working_directory: go{crlf_to_lf_false}
    # language_ids:
    #   atcoder: 3013 # "Go (x.x)"
    #   yukicoder: go # "Go (x.x.x)"
  haskell:
    src: hs/app/{{Pascal}}.hs
    compile:
      bin: hs/target/{{Pascal}}{exe}
      command: [stack, ghc, --, -O2, -o, $bin, $src]
      working_directory: hs
    run:
      command: [$bin]
      working_directory: hs{crlf_to_lf_false}
    # language_ids:
    #   atcoder: 3014      # "Haskell (GHC x.x.x)"
    #   yukicoder: haskell # "Haskell (x.x.x)"
  bash:
    src: bash/{{kebab}}.bash
    run:
      command: [bash, $src]
      working_directory: bash{crlf_to_lf_false}
    # language_ids:
    #   atcoder: 3001 # "Bash (GNU Bash vx.x.x)"
    #   yukicoder: sh # "Bash (Bash x.x.x)"
  python3:
    src: py/{{kebab}}.py
    run:
      command: [{venv_python3}, $src]
      working_directory: py{crlf_to_lf_true}
    language_ids:
      atcoder: 3023      # "Python3 (3.x.x)"
      yukicoder: python3 # "Python3 (3.x.x + numpy x.x.x + scipy x.x.x)"
  java:
    src: java/src/main/java/{{Pascal}}.java
    transpile:
      transpiled: java/build/replaced/{{lower}}/src/Main.java
      command:
        {transpile_java}
      working_directory: java
    compile:
      bin: java/build/replaced/{{lower}}/classes/Main.class
      command: [javac, -d, './build/replaced/{{lower}}/classes', $transpiled]
      working_directory: java
    run:
      command: [java, -classpath, './build/replaced/{{lower}}/classes', Main]
      working_directory: java{crlf_to_lf_true}
    language_ids:
      atcoder: 3016      # "Java8 (OpenJDK 1.8.x)"
      # yukicoder: java8 # "Java8 (openjdk 1.8.x.x)"
  scala:
    src: scala/src/main/scala/{{Pascal}}.scala
    transpile:
      transpiled: scala/target/replaced/{{lower}}/src/Main.scala
      command:
        {transpile_scala}
      working_directory: scala
    compile:
      bin: scala/target/replaced/{{lower}}/classes/Main.class
      command: [scalac, -optimise, -d, './target/replaced/{{lower}}/classes', $transpiled]
      working_directory: scala
    run:
      command: [scala, -classpath, './target/replaced/{{lower}}/classes', Main]
      working_directory: scala{crlf_to_lf_true}
    # language_ids:
    #   atcoder: 3016    # "Scala (x.x.x)"
    #   yukicoder: scala # "Scala(Beta) (x.x.x)"
{csharp}
  text:
    src: txt/{{snake}}.txt
    run:
      command: [cat, $src]
      working_directory: txt{crlf_to_lf_false}
"#,
        console_alt_width = CONSOLE_ALT_WIDTH,
        session_cookies = yaml::escape_string(session_cookies),
        session_dropbox = format_args!(
            "{f}{c}dropbox:\n  {c}  auth: {p}",
            f = if enable_session_dropbox { "" } else { "dropbox : false\n  " },
            c = if enable_session_dropbox { "" } else { "# " },
            p = yaml::escape_string(session_dropbox_auth),
        ),
        shell = SHELL,
        exe = EXE,
        venv_python3 = VENV_PYTHON3,
        transpile_java = TRANSPILE_JAVA,
        transpile_scala = TRANSPILE_SCALA,
        crlf_to_lf_true = CRLF_TO_LF_TRUE,
        crlf_to_lf_false = CRLF_TO_LF_FALSE,
        csharp = CSHARP,
    );
    let path = directory.join(CONFIG_FILE_NAME);
    crate::fs::write(&path, config.as_bytes())?;
    writeln!(stdout, "Wrote to {}", path.display())?;
    stdout.flush().map_err(Into::into)
}

/// Changes attributes.
pub(crate) fn switch(
    mut stdout: impl TermOut,
    mut stderr: impl TermOut,
    directory: &AbsPath,
    service: Option<ServiceName>,
    contest: Option<String>,
    language: Option<String>,
) -> FileResult<()> {
    fn print_change(
        mut stdout: impl WriteAnsi,
        title: &str,
        left_width: usize,
        prev: &Option<String>,
        new: &Option<String>,
    ) -> io::Result<()> {
        let prev = prev.as_ref().map(String::as_str).unwrap_or("~");
        let new = new.as_ref().map(String::as_str).unwrap_or("~");
        stdout.write_str(title)?;
        stdout.with_reset(|o| o.bold()?.write_str(prev))?;
        stdout.write_spaces(left_width - prev.len())?;
        stdout.write_str(" -> ")?;
        stdout.with_reset(|o| o.bold()?.write_str(new))?;
        stdout.write_str("\n")
    }

    let path = crate::fs::find_path(CONFIG_FILE_NAME, directory)?;
    let mut old_yaml = crate::fs::read_to_string(&path)?;
    let old_config = crate::fs::read_yaml::<Config>(&path)?;
    stdout.apply_conf(&old_config.console);
    stderr.apply_conf(&old_config.console);

    let mut m = hashmap!();
    if let Some(service) = service {
        m.insert("service", Cow::from(service.as_static()));
    }
    if let Some(contest) = contest.as_ref() {
        m.insert("contest", Cow::from(contest.clone()));
    }
    if let Some(language) = language.as_ref() {
        if old_config.language.is_some() {
            m.insert("language", Cow::from(language.clone()));
        } else {
            let line_to_insert = format!("language: {}", yaml::escape_string(&language));
            old_yaml = {
                let mut lines = old_yaml.lines().collect::<Vec<_>>();
                let index = if lines.get(0) == Some(&"---") { 1 } else { 0 };
                lines.insert(index, &line_to_insert);
                lines.join("\n")
            };
        }
    }

    let (new_yaml, new_config) = yaml::replace_scalars(&old_yaml, &m)
        .and_then(|new_yaml| {
            let new_config = serde_yaml::from_str(&new_yaml)?;
            Ok((new_yaml, new_config))
        })
        .or_else::<io::Error, _>(|warning| {
            stderr.with_reset(|o| writeln!(o.fg(11)?, "{}", warning))?;
            stderr.flush()?;
            let mut new_config = serde_yaml::from_str::<Config>(&old_yaml)
                .map_err(|e| io::Error::new(io::ErrorKind::Other, e))?;
            new_config.service = service.unwrap_or(new_config.service);
            new_config.contest = contest.unwrap_or(new_config.contest);
            new_config.language = language.or(new_config.language);
            let new_yaml = serde_yaml::to_string(&new_config)
                .map_err(|e| io::Error::new(io::ErrorKind::Other, e))?;
            Ok((new_yaml, new_config))
        })?;

    let s1 = Some(format!("{:?}", old_config.service.as_static()));
    let s2 = Some(format!("{:?}", new_config.service.as_static()));
    let c1 = Some(format!("{:?}", old_config.contest));
    let c2 = Some(format!("{:?}", new_config.contest));
    let l1 = old_config.language.as_ref().map(|l| format!("{:?}", l));
    let l2 = new_config.language.as_ref().map(|l| format!("{:?}", l));
    let w = [
        s1.as_ref().map(|s| stdout.str_width(s)).unwrap_or(1),
        c1.as_ref().map(|s| stdout.str_width(s)).unwrap_or(1),
        l1.as_ref().map(|s| stdout.str_width(s)).unwrap_or(1),
    ]
    .iter()
    .cloned()
    .max()
    .unwrap();
    print_change(&mut stdout, "service:  ", w, &s1, &s2)?;
    print_change(&mut stdout, "contest:  ", w, &c1, &c2)?;
    print_change(&mut stdout, "language: ", w, &l1, &l2)?;
    crate::fs::write(&path, new_yaml.as_bytes())?;

    writeln!(stdout, "Saved to {}", path.display())?;
    stdout.flush().map_err(Into::into)
}

/// Config.
#[derive(Serialize, Deserialize)]
pub(crate) struct Config {
    #[serde(default)]
    service: ServiceName,
    contest: String,
    language: Option<String>,
    #[serde(default)]
    console: Console,
    testfile_path: TemplateBuilder<AbsPathBuf>,
    session: Session,
    judge: Judge,
    #[serde(default)]
    services: BTreeMap<ServiceName, ServiceConfig>,
    #[serde(default)]
    interactive: HashMap<String, Language>,
    languages: HashMap<String, Language>,
    #[serde(skip)]
    base_dir: AbsPathBuf,
}

impl Config {
    pub(crate) fn load(
        service: impl Into<Option<ServiceName>>,
        contest: impl Into<Option<String>>,
        dir: &AbsPath,
    ) -> FileResult<Self> {
        let path = crate::fs::find_path(CONFIG_FILE_NAME, dir)?;
        let mut config = crate::fs::read_yaml::<Self>(&path)?;
        config.base_dir = path.parent().unwrap().to_owned();
        config.service = service.into().unwrap_or(config.service);
        config.contest = contest.into().unwrap_or(config.contest);
        Ok(config)
    }

    /// Gets `service`.
    pub(crate) fn service(&self) -> ServiceName {
        self.service
    }

    /// Gets `contest`.
    pub(crate) fn contest(&self) -> &str {
        &self.contest
    }

    pub(crate) fn console(&self) -> &Console {
        &self.console
    }

    /// Gets `session.timeout`.
    pub(crate) fn session_timeout(&self) -> Option<Duration> {
        self.session.timeout
    }

    pub(crate) fn session_silent(&self) -> bool {
        self.session.silent
    }

    /// Gets `session.cookies` embedding "service" and "base_dir".
    pub(crate) fn session_cookies(&self) -> Template<AbsPathBuf> {
        self.session
            .cookies
            .build(self.base_dir.clone())
            .strings(hashmap!("service".to_owned() => self.service.to_string()))
    }

    pub(crate) fn session_dropbox_auth(&self) -> Option<Template<AbsPathBuf>> {
        match &self.session.dropbox {
            Dropbox::None => None,
            Dropbox::Some { auth } => Some(
                auth.build(self.base_dir.clone())
                    .strings(hashmap!("service".to_owned() => self.service.to_string())),
            ),
        }
    }

    pub(crate) fn judge_jobs(&self) -> NonZeroUsize {
        self.judge.jobs
    }

    pub(crate) fn download_destinations(
        &self,
        ext: Option<SuiteFileExtension>,
    ) -> DownloadDestinations {
        let scraped = self
            .testfile_path
            .build(self.base_dir.clone())
            .insert_string("service", self.service.as_static())
            .insert_string("contest", &self.contest);
        let text_file_dir = self
            .session
            .download
            .text_file_dir
            .build(self.base_dir.clone())
            .insert_string("service", self.service.as_static())
            .insert_string("contest", &self.contest);
        let ext = ext.unwrap_or(self.session.download.extension);
        DownloadDestinations::new(scraped, text_file_dir, ext)
    }

    pub(crate) fn testcase_loader(&self) -> TestCaseLoader {
        let path = self
            .testfile_path
            .build(self.base_dir.clone())
            .insert_string("service", self.service.as_static())
            .insert_string("contest", &self.contest);
        TestCaseLoader::new(
            path,
            &self.judge.testfile_extensions,
            self.interactive_tester_transpilations(),
            self.interactive_tester_compilations(),
            self.interactive_testers(),
        )
    }

    pub(crate) fn src_paths(&self) -> HashMap<&str, Template<AbsPathBuf>> {
        let vars = self.vars_for_langs(None);
        let mut templates = hashmap!();
        for lang in self.languages.values() {
            if let Some(lang_id) = lang.language_ids.get(&ServiceName::Atcoder) {
                let template = lang.src.build(self.base_dir.clone()).insert_strings(&vars);
                templates.insert(lang_id.as_str(), template);
            }
        }
        templates
    }

    pub(crate) fn src_to_submit(&self, lang: Option<&str>) -> ConfigResult<Template<AbsPathBuf>> {
        let lang = find_language(&self.languages, self.lang_name(lang)?)?;
        let builder = match &lang.transpile {
            None => &lang.src,
            Some(transpile) => &transpile.transpiled,
        };
        let (base_dir, vars) = (self.base_dir.clone(), self.vars_for_langs(None));
        Ok(builder.build(base_dir).insert_strings(&vars))
    }

    pub(crate) fn lang_id(&self, service: ServiceName, lang: Option<&str>) -> Option<&str> {
        let lang = find_language(&self.languages, self.lang_name(lang).ok()?).ok()?;
        lang.language_ids.get(&service).map(String::as_str)
    }

    pub(crate) fn solver_compilation(
        &self,
        lang: Option<&str>,
    ) -> ConfigResult<Option<Template<CompilationCommand>>> {
        let lang = find_language(&self.languages, self.lang_name(lang)?)?;
        Ok(self.compilation_command(lang))
    }

    pub(crate) fn solver_transpilation(
        &self,
        lang: Option<&str>,
    ) -> ConfigResult<Option<Template<TranspilationCommand>>> {
        let lang = find_language(&self.languages, self.lang_name(lang)?)?;
        Ok(self.transpilation_command(lang))
    }

    pub(crate) fn solver(&self, lang: Option<&str>) -> ConfigResult<Template<JudgingCommand>> {
        let lang = find_language(&self.languages, self.lang_name(lang)?)?;
        Ok(self.judge_command(lang))
    }

    fn interactive_tester_transpilations(&self) -> HashMap<String, Template<TranspilationCommand>> {
        self.interactive
            .iter()
            .filter_map(|(name, conf)| {
                self.transpilation_command(conf)
                    .map(|t| (name.to_owned(), t))
            })
            .collect()
    }

    fn interactive_tester_compilations(&self) -> HashMap<String, Template<CompilationCommand>> {
        self.interactive
            .iter()
            .filter_map(|(name, conf)| self.compilation_command(conf).map(|t| (name.to_owned(), t)))
            .collect()
    }

    fn interactive_testers(&self) -> HashMap<String, Template<JudgingCommand>> {
        self.interactive
            .iter()
            .map(|(name, conf)| (name.clone(), self.judge_command(&conf)))
            .collect()
    }

    fn transpilation_command(&self, lang: &Language) -> Option<Template<TranspilationCommand>> {
        lang.transpile.as_ref().map(|transpile| {
            transpile
                .command
                .build(TranspilationCommandRequirements {
                    base_dir: self.base_dir.clone(),
                    shell: self.judge.shell.clone(),
                    working_dir: transpile.working_directory.clone(),
                    src: lang.src.clone(),
                    transpiled: transpile.transpiled.clone(),
                })
                .insert_strings(&self.vars_for_langs(None))
        })
    }

    fn compilation_command(&self, lang: &Language) -> Option<Template<CompilationCommand>> {
        lang.compile.as_ref().map(|compile| {
            compile
                .command
                .build(CompilationCommandRequirements {
                    base_dir: self.base_dir.clone(),
                    shell: self.judge.shell.clone(),
                    working_dir: compile.working_directory.clone(),
                    src: lang.src.clone(),
                    transpiled: lang.transpile.as_ref().map(|e| e.transpiled.clone()),
                    bin: compile.bin.clone(),
                })
                .insert_strings(&self.vars_for_langs(None))
        })
    }

    fn judge_command(&self, lang: &Language) -> Template<JudgingCommand> {
        lang.run
            .command
            .build(JudgingCommandRequirements {
                base_dir: self.base_dir.clone(),
                shell: self.judge.shell.clone(),
                working_dir: lang.run.working_directory.clone(),
                src: lang.src.clone(),
                bin: lang.compile.as_ref().map(|e| e.bin.clone()),
                transpiled: lang.transpile.as_ref().map(|e| e.transpiled.clone()),
                crlf_to_lf: lang.run.crlf_to_lf,
            })
            .insert_strings(&self.vars_for_langs(None))
    }

    fn lang_name<'a>(&'a self, name: Option<&'a str>) -> ConfigResult<&'a str> {
        name.or_else(|| {
            self.services
                .get(&self.service)
                .and_then(|s| s.language.as_ref())
                .map(String::as_str)
        })
        .or_else(|| self.language.as_ref().map(String::as_str))
        .ok_or_else(|| ConfigErrorKind::PropertyNotSet("language").into())
    }

    fn vars_for_langs(&self, service: impl Into<Option<ServiceName>>) -> HashMap<&str, &str> {
        let vars_in_service = self
            .services
            .get(&service.into().unwrap_or(self.service))
            .map(|s| &s.variables);
        let mut vars = hashmap!("service" => self.service.as_static(), "contest" => &self.contest);
        if let Some(vars_in_service) = vars_in_service {
            for (k, v) in vars_in_service {
                vars.insert(k, v);
            }
        }
        vars
    }
}

fn find_language<'a>(
    langs: &HashMap<String, Language>,
    default_lang: impl Into<Option<&'a str>>,
) -> ConfigResult<&Language> {
    let name = default_lang
        .into()
        .ok_or_else(|| ConfigErrorKind::LanguageNotSpecified)?;
    langs
        .get(name)
        .ok_or_else(|| ConfigErrorKind::NoSuchLanguage(name.to_owned()).into())
}

#[derive(Default, Serialize, Deserialize)]
pub struct Console {
    #[serde(default)]
    pub(crate) cjk: bool,
    pub(crate) alt_width: Option<usize>,
}

#[derive(Serialize, Deserialize)]
pub(crate) struct Session {
    #[serde(
        serialize_with = "time::ser_secs",
        deserialize_with = "time::de_secs",
        default
    )]
    timeout: Option<Duration>,
    #[serde(default)]
    silent: bool,
    cookies: TemplateBuilder<AbsPathBuf>,
    #[serde(default)]
    dropbox: Dropbox,
    download: Download,
}

enum Dropbox {
    None,
    Some { auth: TemplateBuilder<AbsPathBuf> },
}

impl Default for Dropbox {
    fn default() -> Self {
        Dropbox::None
    }
}

impl Serialize for Dropbox {
    fn serialize<S: Serializer>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error> {
        match self {
            Dropbox::None => serializer.serialize_bool(false),
            Dropbox::Some { auth } => {
                let mut map = serializer.serialize_map(Some(1))?;
                map.serialize_entry("auth", auth)?;
                map.end()
            }
        }
    }
}

impl<'de> Deserialize<'de> for Dropbox {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> std::result::Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Repr {
            Bool(bool),
            Some { auth: TemplateBuilder<AbsPathBuf> },
        }

        match Repr::deserialize(deserializer)? {
            Repr::Bool(true) => Err(serde::de::Error::custom(
                "expected `false` or `{ auth: <string> }`",
            )),
            Repr::Bool(false) => Ok(Dropbox::None),
            Repr::Some { auth } => Ok(Dropbox::Some { auth }),
        }
    }
}

#[derive(Serialize, Deserialize)]
struct Download {
    extension: SuiteFileExtension,
    text_file_dir: TemplateBuilder<AbsPathBuf>,
}

#[derive(Serialize, Deserialize)]
struct Judge {
    jobs: NonZeroUsize,
    testfile_extensions: BTreeSet<SuiteFileExtension>,
    shell: HashMap<String, Vec<TemplateBuilder<OsString>>>,
}

#[derive(Serialize, Deserialize)]
struct ServiceConfig {
    language: Option<String>,
    variables: HashMap<String, String>,
}

#[derive(Serialize, Deserialize)]
struct Language {
    src: TemplateBuilder<AbsPathBuf>,
    #[serde(skip_serializing_if = "Option::is_none")]
    transpile: Option<Transpile>,
    #[serde(skip_serializing_if = "Option::is_none")]
    compile: Option<Compile>,
    run: Run,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    language_ids: BTreeMap<ServiceName, String>,
}

#[derive(Serialize, Deserialize)]
struct Transpile {
    transpiled: TemplateBuilder<AbsPathBuf>,
    command: TemplateBuilder<TranspilationCommand>,
    #[serde(default)]
    working_directory: TemplateBuilder<AbsPathBuf>,
}

#[derive(Serialize, Deserialize)]
struct Compile {
    bin: TemplateBuilder<AbsPathBuf>,
    command: TemplateBuilder<CompilationCommand>,
    #[serde(default)]
    working_directory: TemplateBuilder<AbsPathBuf>,
}

#[derive(Serialize, Deserialize)]
struct Run {
    command: TemplateBuilder<JudgingCommand>,
    #[serde(default)]
    working_directory: TemplateBuilder<AbsPathBuf>,
    #[serde(default)]
    crlf_to_lf: bool,
}
