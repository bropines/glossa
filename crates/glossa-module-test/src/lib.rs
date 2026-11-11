//! The module test harness behind `glossa-cli module test`.
//!
//! For one module folder it:
//!
//! 1. checks `module.toml` and the files beside it ([`lint`]): unknown keys,
//!    language codes, options, permissions, the entry file, hard-coded
//!    secrets, size;
//! 2. loads the module through [`Registry::load_module`], the same code the
//!    program uses;
//! 3. runs the cases in `tests/*.toml` / `tests/*.json` ([`fixture`])
//!    through the provider API, answering the module's HTTP requests from
//!    recorded responses ([`net`]) so the cases run offline;
//! 4. checks each result's shape and the case's expectations ([`checks`]),
//!    and times everything.
//!
//! Nothing here is specific to a front end; the CLI only parses flags and
//! prints [`ModuleReport`]s.

pub mod checks;
pub mod fixture;
pub mod lint;
pub mod net;

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use glossa_core::ocr::{ImageSource, OcrGranularity};
use glossa_core::{DictRequest, LangId, LangPair, OcrRequest, SpeakRequest, TranslateRequest};
use glossa_modules::provider::resolve_options;
use glossa_modules::wire::{ActionEvent, ActionParams};
use glossa_modules::{CancelToken, Host, OptionValues, Provider, Registry, RuntimeContext};
use serde::Serialize;

use checks::Outcome;
use fixture::{Case, Exchange, FixtureFile, ImageSpec, Op};

/// Where the cases' HTTP requests go.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Network {
    /// Recorded responses only; anything else fails. The default, and what
    /// CI runs.
    #[default]
    Replay,
    /// The real network.
    Live,
    /// The real network, writing what came back to
    /// `tests/recorded/<fixture>/<case>.json`.
    Record,
}

/// How to run.
#[derive(Debug, Clone)]
pub struct Settings {
    pub network: Network,
    /// The budget for one call, as the program's `[modules] call_timeout`.
    pub timeout: Duration,
    /// Only check, do not load or call the module.
    pub check_only: bool,
    /// A module that cannot run here (no interpreter, not built) fails
    /// instead of being skipped.
    pub require_runtimes: bool,
    /// Warnings fail the module too.
    pub strict: bool,
    /// Only the cases whose name contains this.
    pub case_filter: Option<String>,
    /// Replay the hand-written `[[case.http]]` / `[[http]]` entries only,
    /// ignoring `tests/recorded/`.
    pub ignore_recordings: bool,
    /// Option values over every fixture's, for live runs with a real key.
    /// Every text value among them is kept out of recordings.
    pub options: OptionValues,
    pub python: String,
    pub java: String,
    /// Where module assets are downloaded, as the program has it
    /// (`Paths::models_dir`), so files the user already downloaded are
    /// found. `None`: a scratch folder.
    pub models_dir: Option<PathBuf>,
    /// Download a module's missing required assets (and the managed Python
    /// and packages a Python module needs) before running it, instead of
    /// skipping the module.
    pub download_assets: bool,
    /// Where the managed Python, `uv` and the modules' environments are, as
    /// the program has them, so a Python the user already downloaded is
    /// used. `None`: a scratch folder.
    pub python_dirs: Option<glossa_modules::python_env::PythonDirs>,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            network: Network::Replay,
            timeout: Duration::from_secs(30),
            check_only: false,
            require_runtimes: false,
            strict: false,
            case_filter: None,
            ignore_recordings: false,
            options: OptionValues::new(),
            python: String::new(),
            java: String::new(),
            models_dir: None,
            download_assets: false,
            python_dirs: None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    Note,
    Warning,
    Error,
}

/// Something wrong (or worth knowing) that is not tied to one case.
#[derive(Debug, Clone, Serialize)]
pub struct Finding {
    pub severity: Severity,
    pub message: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Status {
    Pass,
    Fail,
    Skip,
}

#[derive(Debug, Clone, Serialize)]
pub struct CaseReport {
    /// The fixture file, relative to the module.
    pub file: String,
    pub name: String,
    pub op: Op,
    pub status: Status,
    pub ms: u64,
    /// The HTTP requests the call made, as `METHOD url` without the query.
    pub requests: Vec<String>,
    /// What came back, in one line.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<String>,
    /// Why it failed or was skipped.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub problems: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ModuleReport {
    pub dir: PathBuf,
    pub id: String,
    pub runtime: String,
    pub status: Status,
    /// Why the module was skipped.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub skipped: Option<String>,
    pub size_bytes: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub load_ms: Option<u64>,
    pub findings: Vec<Finding>,
    pub cases: Vec<CaseReport>,
}

impl ModuleReport {
    fn new(dir: &Path) -> Self {
        ModuleReport {
            dir: dir.to_owned(),
            id: String::new(),
            runtime: String::new(),
            status: Status::Pass,
            skipped: None,
            size_bytes: 0,
            load_ms: None,
            findings: Vec::new(),
            cases: Vec::new(),
        }
    }

    pub fn count(&self, severity: Severity) -> usize {
        self.findings.iter().filter(|f| f.severity == severity).count()
    }

    fn settle(&mut self, s: &Settings) {
        // Messages name files under the absolute folder the module was
        // loaded from; show them relative to the module instead.
        if let Ok(abs) = std::path::absolute(&self.dir) {
            let prefix = format!("{}{}", abs.display(), std::path::MAIN_SEPARATOR);
            let tidy = |m: &mut String| *m = m.replace(&prefix, "");
            self.findings.iter_mut().for_each(|f| tidy(&mut f.message));
            self.cases.iter_mut().flat_map(|c| c.problems.iter_mut().chain(c.result.iter_mut())).for_each(tidy);
            if let Some(why) = self.skipped.as_mut() {
                tidy(why);
            }
        }
        let failed = self.count(Severity::Error) > 0
            || (s.strict && self.count(Severity::Warning) > 0)
            || self.cases.iter().any(|c| c.status == Status::Fail)
            || (s.require_runtimes && self.skipped.is_some());
        self.status = if failed {
            Status::Fail
        } else if self.skipped.is_some() {
            Status::Skip
        } else {
            Status::Pass
        };
    }

    /// The report as text, for a terminal.
    pub fn render(&self) -> String {
        let mut out = String::new();
        let label = match self.status {
            Status::Pass => "PASS",
            Status::Fail => "FAIL",
            Status::Skip => "SKIP",
        };
        let id = if self.id.is_empty() { "?" } else { &self.id };
        let mut head = format!("{label} {id} ({}) {}", self.runtime, self.dir.display());
        if let Some(ms) = self.load_ms {
            head.push_str(&format!("  load {ms} ms"));
        }
        head.push_str(&format!("  {}", human_size(self.size_bytes)));
        out.push_str(&head);
        out.push('\n');
        if let Some(why) = &self.skipped {
            out.push_str(&format!("    skipped: {why}\n"));
        }
        let mut findings = self.findings.clone();
        findings.sort_by_key(|f| std::cmp::Reverse(f.severity));
        for f in &findings {
            let tag = match f.severity {
                Severity::Error => "error",
                Severity::Warning => "warning",
                Severity::Note => "note",
            };
            out.push_str(&format!("    {tag}: {}\n", f.message));
        }
        for c in &self.cases {
            let mark = match c.status {
                Status::Pass => "ok  ",
                Status::Fail => "FAIL",
                Status::Skip => "skip",
            };
            let http = if c.requests.is_empty() { String::new() } else { format!(", {} http", c.requests.len()) };
            out.push_str(&format!("    {mark} {}: {} [{}] {} ms{http}", c.file, c.name, c.op.name(), c.ms));
            if let Some(r) = &c.result {
                out.push_str(&format!(" -> {r}"));
            }
            out.push('\n');
            for p in &c.problems {
                out.push_str(&format!("         {p}\n"));
            }
        }
        out
    }
}

fn human_size(bytes: u64) -> String {
    if bytes >= 1_048_576 {
        format!("{:.1} MB", bytes as f64 / 1_048_576.0)
    } else if bytes >= 1024 {
        format!("{} KB", bytes / 1024)
    } else {
        format!("{bytes} B")
    }
}

/// The module folders under `root`: `root` itself when it has a
/// `module.toml`, else every folder below it that has one (not looking
/// inside a module, nor in build folders).
pub fn find_modules(root: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    fn visit(dir: &Path, out: &mut Vec<PathBuf>) {
        if dir.join("module.toml").is_file() {
            out.push(dir.to_owned());
            return;
        }
        let Ok(read) = std::fs::read_dir(dir) else { return };
        let mut subdirs: Vec<PathBuf> = read
            .flatten()
            .filter(|e| e.path().is_dir())
            .filter(|e| {
                let name = e.file_name().to_string_lossy().into_owned();
                !name.starts_with('.') && !["target", "node_modules", "__pycache__", "build"].contains(&name.as_str())
            })
            .map(|e| e.path())
            .collect();
        subdirs.sort();
        for d in subdirs {
            visit(&d, out);
        }
    }
    visit(root, &mut out);
    out
}

/// Check, load and run one module.
pub fn test_module(dir: &Path, s: &Settings) -> ModuleReport {
    let mut report = ModuleReport::new(dir);
    // The program always hands the runtimes an absolute folder; a relative
    // one would be resolved twice by runtimes that also set the process's
    // working directory to it.
    let absolute = std::path::absolute(dir).unwrap_or_else(|_| dir.to_owned());
    let dir = absolute.as_path();
    let ctx = RuntimeContext {
        python: s.python.clone(),
        java: s.java.clone(),
        call_timeout: s.timeout,
        cache_dir: std::env::temp_dir().join("glossa-module-test"),
        models_dir: s.models_dir.clone().unwrap_or_else(|| std::env::temp_dir().join("glossa-module-test-models")),
        python_dirs: s.python_dirs.clone().unwrap_or_default(),
        // Each case stands alone: one that reports a rate limit must not
        // make the next ones wait out the cooling-off period.
        rate_limit_cooldown: false,
        ..Default::default()
    };

    let lint = lint::check(dir, &ctx);
    report.findings = lint.findings;
    report.size_bytes = lint.size;
    let Some(manifest) = lint.manifest else {
        report.settle(s);
        return report;
    };
    report.id = manifest.module.id.clone();
    report.runtime = manifest.module.runtime.label().to_owned();

    // The fixtures are checked even when nothing runs.
    let mut fixtures: Vec<(PathBuf, FixtureFile)> = Vec::new();
    for path in fixture::files(dir) {
        match fixture::load(&path) {
            Ok(f) => fixtures.push((path, f)),
            Err(e) => report.findings.push(Finding::error(e)),
        }
    }
    let ops: Vec<Op> = fixtures.iter().flat_map(|(_, f)| f.cases.iter().map(|c| c.op)).collect();
    if fixtures.is_empty() {
        report.findings.push(Finding::warning(format!(
            "no fixtures in {}: only the manifest was checked (see docs/modules/testing.md)",
            fixture::dir(dir).display()
        )));
    } else {
        for op in Op::ALL {
            let declared = manifest.capabilities.iter().any(|c| c == op.capability());
            let tested = ops.contains(&op);
            if declared && !tested {
                report.findings.push(Finding::warning(format!(
                    "capability {:?} has no case; add one with op = {:?}",
                    op.capability(),
                    op.name()
                )));
            } else if tested && !declared {
                report.findings.push(Finding::error(format!(
                    "a case calls {:?} but the manifest does not declare capability {:?}",
                    op.name(),
                    op.capability()
                )));
            }
        }
        // Actions: each case names a declared one, and each one the module
        // runs has a case.
        let cases = || fixtures.iter().flat_map(|(_, f)| f.cases.iter()).filter(|c| c.op == Op::Action);
        for c in cases() {
            match c.action.as_deref() {
                None => {
                    report.findings.push(Finding::error(format!("case {:?}: an action case needs `action`", c.name)))
                }
                Some(id) if !manifest.actions.iter().any(|a| a.id == id) => report.findings.push(Finding::error(
                    format!("case {:?}: action {id:?} is not in the manifest's [[actions]]", c.name),
                )),
                Some(_) => {}
            }
        }
        for a in &manifest.actions {
            let handler = a.kind() == glossa_modules::actions::ActionKind::Handler;
            if handler && !cases().any(|c| c.action.as_deref() == Some(a.id.as_str())) {
                report.findings.push(Finding::warning(format!(
                    "action {:?} has no case; add one with op = \"action\", action = {:?}",
                    a.id, a.id
                )));
            }
        }
    }

    if s.check_only || report.count(Severity::Error) > 0 {
        report.settle(s);
        return report;
    }
    if let Some(why) = lint.missing {
        report.skipped = Some(why);
        report.settle(s);
        return report;
    }

    let started = Instant::now();
    let entry = match Registry::load_module(dir, &ctx) {
        Ok(e) => e,
        Err(why) => {
            report.findings.push(Finding::error(format!("the module does not load: {why}")));
            report.settle(s);
            return report;
        }
    };
    report.load_ms = Some(started.elapsed().as_millis() as u64);
    // A module whose files are not downloaded is listed but not started,
    // in the program as here: fetch them when asked to, else skip it the
    // way a missing interpreter is skipped.
    if let Some(assets) = entry.assets.as_ref().filter(|a| !a.ready()) {
        if s.download_assets {
            let http = glossa_net::HttpClient::plain();
            if let Err(e) = assets.download(&http, &CancelToken::new(), &|_| {}) {
                report.findings.push(Finding::error(format!("downloading the module's files failed: {e}")));
                report.settle(s);
                return report;
            }
        } else {
            report.skipped = Some(format!(
                "the module's files are not downloaded ({}); run `glossa-cli module assets download {}`, or test with --download",
                assets.missing().join(", "),
                report.id
            ));
            report.settle(s);
            return report;
        }
    }
    // Likewise a Python module whose interpreter or packages are not there.
    if let Some(env) = entry.python.as_ref() {
        use glossa_modules::python_env::EnvState;
        match env.state() {
            EnvState::Ready | EnvState::Unknown => {}
            EnvState::Impossible(why) => {
                report.skipped = Some(why);
                report.settle(s);
                return report;
            }
            EnvState::Missing(missing) if s.download_assets => {
                let http = glossa_net::HttpClient::plain();
                if let Err(e) = env.install(&http, &CancelToken::new(), &|_| {}) {
                    report.findings.push(Finding::error(format!("getting {} failed: {e}", missing.join(", "))));
                    report.settle(s);
                    return report;
                }
            }
            EnvState::Missing(missing) => {
                report.skipped = Some(format!(
                    "the module needs {}; run `glossa-cli python install {}`, or test with --download",
                    missing.join(", "),
                    report.id
                ));
                report.settle(s);
                return report;
            }
        }
    }
    let provider = entry.provider;
    // Everything given on the command line is treated as secret, whatever
    // the option's type: a key in a `text` option is still a key.
    let secrets: Vec<String> = s.options.values().filter_map(|v| v.as_str()).map(str::to_owned).collect();

    for (path, file) in &fixtures {
        let shown = path.strip_prefix(dir).unwrap_or(path).display().to_string();
        for case in &file.cases {
            if s.case_filter.as_deref().is_some_and(|f| !case.name.contains(f)) {
                continue;
            }
            let r = run_case(provider.as_ref(), path, file, case, s, &secrets);
            report.cases.push(CaseReport { file: shown.clone(), ..r });
        }
    }
    provider.suspend();
    report.settle(s);
    report
}

fn run_case(
    provider: &dyn Provider,
    fixture_path: &Path,
    file: &FixtureFile,
    case: &Case,
    s: &Settings,
    secrets: &[String],
) -> CaseReport {
    let mut report = CaseReport {
        file: String::new(),
        name: case.name.clone(),
        op: case.op,
        status: Status::Fail,
        ms: 0,
        requests: Vec::new(),
        result: None,
        problems: Vec::new(),
    };
    let base = fixture_path.parent().unwrap_or(Path::new("."));
    if case.live && s.network == Network::Replay {
        report.status = Status::Skip;
        report.problems.push("needs the real service; run with --live".into());
        return report;
    }

    let mut user = file.options.clone();
    user.extend(case.options.clone());
    user.extend(s.options.clone());
    let options = resolve_options(provider.options(), Some(&user));

    let recording = fixture::recording_path(fixture_path, &case.name);
    let replay = match s.network {
        Network::Replay => {
            // A recording is the service's own, newer answer, so it is tried
            // first; the hand-written entries answer what it does not cover.
            let mut exchanges: Vec<Exchange> = Vec::new();
            if !s.ignore_recordings {
                if let Ok(text) = std::fs::read_to_string(&recording) {
                    match serde_json::from_str::<fixture::Recording>(&text) {
                        Ok(r) => exchanges.extend(r.exchanges),
                        Err(e) => {
                            report.problems.push(format!("{}: {e}", recording.display()));
                            return report;
                        }
                    }
                }
            }
            exchanges.extend(case.http.iter().cloned());
            exchanges.extend(file.http.iter().cloned());
            match net::Replay::new(&exchanges, base) {
                Ok(r) => Some(r),
                Err(e) => {
                    report.problems.push(e);
                    return report;
                }
            }
        }
        _ => None,
    };
    let live = (s.network != Network::Replay).then(|| net::Live::new(s.network == Network::Record, secrets.to_vec()));
    let http: &dyn glossa_net::Http = match (&replay, &live) {
        (Some(r), _) => r,
        (None, Some(l)) => l,
        (None, None) => unreachable!("one of the two is always set"),
    };

    let request = match build(case, base) {
        Ok(r) => r,
        Err(e) => {
            report.problems.push(e);
            return report;
        }
    };

    let cancel = CancelToken::new();
    let host = Host { http, options: &options, ui_language: LangId::ENGLISH, preferred_domain: "com", cancel: &cancel };
    let started = Instant::now();
    let result = call(provider, &host, request);
    // An action may only name the module's own fields.
    if let Ok(Outcome::Action(r, _)) = &result {
        for key in r.choices.keys().chain(r.errors.keys()).chain(r.options.keys()) {
            if !provider.options().iter().any(|o| o.key == *key) {
                report.problems.push(format!("the reply names {key:?}, which is not one of the module's options"));
            }
        }
    }
    report.ms = started.elapsed().as_millis() as u64;
    report.requests = match (&replay, &live) {
        (Some(r), _) => r.requests(),
        (_, Some(l)) => l.requests(),
        _ => Vec::new(),
    }
    .iter()
    .map(|r| glossa_net::redact(r).to_owned())
    .collect();

    if let Some(l) = &live {
        let recorded = l.recorded();
        if s.network == Network::Record && !recorded.is_empty() {
            if let Err(e) = write_recording(&recording, recorded) {
                report.problems.push(format!("cannot write {}: {e}", recording.display()));
            }
        }
    }

    let e = &case.expect;
    match result {
        Ok(outcome) => {
            report.result = Some(outcome.preview());
            if let Some(want) = &e.error {
                report.problems.push(format!("expected a {want} error, got a result"));
            } else {
                report.problems.extend(checks::shape(&outcome));
                report.problems.extend(checks::expectations(&outcome, e));
            }
        }
        Err(err) => {
            let kind = checks::error_kind(&err);
            match &e.error {
                Some(want) if checks::error_matches(&err, want) => {
                    report.result = Some(format!("error {kind}: {err}"));
                    if want == "not_found" && kind == "module" && e.error_contains.is_none() {
                        // A dictionary's not_found reaches the program as a
                        // plain module error, the same as a crash; only the
                        // message tells them apart.
                        report.problems.push(
                            "a dictionary's not_found arrives as a plain module error, like a crash; add \
                             error_contains with words from the module's not_found message"
                                .into(),
                        );
                    }
                    if let Some(part) = &e.error_contains {
                        if !err.to_string().contains(part.as_str()) {
                            report.problems.push(format!("expected the error to mention {part:?}"));
                        }
                    }
                }
                Some(want) => {
                    let at = err.source_place().map(|p| format!(" (at {p})")).unwrap_or_default();
                    report.problems.push(format!("expected a {want} error, got {kind}: {err}{at}"))
                }
                None => {
                    let hint = match &err {
                        glossa_core::Error::BadResponse { detail, .. } if detail.contains(" reply: ") => {
                            " (the reply does not match the protocol; see docs/modules/protocol.md)"
                        }
                        glossa_core::Error::ModuleTimeout { .. } => {
                            " (stdout not flushed, something other than a frame written to it, or a slow call)"
                        }
                        _ => "",
                    };
                    // Where the module threw, for its author (the editor links it).
                    let at = err.source_place().map(|p| format!(" (at {p})")).unwrap_or_default();
                    report.problems.push(format!("failed with {kind}: {err}{hint}{at}"));
                }
            }
        }
    }
    if let Some(max) = e.max_ms {
        if report.ms > max {
            report.problems.push(format!("took {} ms, more than max_ms = {max}", report.ms));
        }
    }
    report.status = if report.problems.is_empty() { Status::Pass } else { Status::Fail };
    report
}

fn write_recording(path: &Path, exchanges: Vec<Exchange>) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut text = serde_json::to_string_pretty(&fixture::Recording { exchanges }).map_err(std::io::Error::other)?;
    text.push('\n');
    std::fs::write(path, text)
}

enum Request {
    Translate(TranslateRequest),
    Detect(String),
    Dictionary(DictRequest),
    Speak(SpeakRequest),
    Ocr(OcrRequest, Option<(u32, u32)>),
    Action(ActionParams),
}

fn lang(code: &str, field: &str) -> Result<LangId, String> {
    LangId::from_code(code).ok_or_else(|| format!("{field} = {code:?} is not a language code"))
}

fn build(case: &Case, base: &Path) -> Result<Request, String> {
    let needs_text = !matches!(case.op, Op::Ocr | Op::Action);
    if needs_text && case.text.is_empty() {
        return Err(format!("a {} case needs `text`", case.op.name()));
    }
    Ok(match case.op {
        Op::Translate => {
            let pair = LangPair::new(lang(&case.from, "from")?, lang(&case.to, "to")?);
            Request::Translate(TranslateRequest::new(case.text.clone(), pair))
        }
        Op::Detect => Request::Detect(case.text.clone()),
        Op::Dictionary => Request::Dictionary(DictRequest {
            text: case.text.clone(),
            from: lang(&case.from, "from")?,
            to: lang(&case.to, "to")?,
            exact: case.exact.unwrap_or(true),
        }),
        Op::Speak => Request::Speak(SpeakRequest {
            text: case.text.clone(),
            lang: lang(case.lang.as_deref().unwrap_or("en"), "lang")?,
            slow: case.slow,
        }),
        Op::Ocr => {
            let bytes = match &case.image {
                None => return Err("an ocr case needs `image`".into()),
                Some(ImageSpec::File(f)) => {
                    std::fs::read(base.join(f)).map_err(|e| format!("image {}: {e}", base.join(f).display()))?
                }
                Some(ImageSpec::Drawn { width, height, lines }) => draw(*width, *height, *lines)?,
            };
            let size = glossa_modules::protocol::image_dimensions(&bytes);
            let mut req = OcrRequest::new(ImageSource::Bytes(bytes));
            req.lang = lang(case.lang.as_deref().unwrap_or("auto"), "lang")?;
            req.translate_to = case.translate_to.as_deref().map(|t| lang(t, "translate_to")).transpose()?;
            req.granularity = match case.granularity.as_deref().unwrap_or("words") {
                "full_text" => OcrGranularity::FullText,
                "blocks" => OcrGranularity::Blocks,
                "lines" => OcrGranularity::Lines,
                "words" => OcrGranularity::Words,
                other => return Err(format!("granularity {other:?}: use full_text, blocks, lines or words")),
            };
            Request::Ocr(req, size)
        }
        Op::Action => {
            let action = case.action.clone().ok_or("an action case needs `action`")?;
            Request::Action(ActionParams {
                event: Some(ActionEvent {
                    kind: "click".into(),
                    element: Some(action.clone()),
                    item: case.item.clone(),
                }),
                action,
                panel: case.panel.clone(),
                state: case.state.clone(),
                ui_lang: case.ui_lang.clone().unwrap_or_else(|| "en".into()),
                ..Default::default()
            })
        }
    })
}

fn call(p: &dyn Provider, host: &Host, request: Request) -> glossa_core::Result<Outcome> {
    Ok(match request {
        Request::Translate(r) => Outcome::Translation(p.translate(host, &r)?),
        Request::Detect(t) => Outcome::Detection(p.detect(host, &t)?),
        Request::Dictionary(r) => Outcome::Article(p.dictionary(host, &r)?),
        Request::Speak(r) => Outcome::Audio(p.speak(host, &r)?),
        Request::Ocr(r, size) => Outcome::Ocr { result: p.recognize(host, &r)?, image_size: size },
        Request::Action(params) => {
            let updates = std::sync::Mutex::new(Vec::new());
            let reply = p.action_with_updates(host, params, &|u| {
                if let Ok(mut list) = updates.lock() {
                    list.push(u);
                }
            })?;
            Outcome::Action(reply, updates.into_inner().unwrap_or_default())
        }
    })
}

/// A white PNG with `lines` dark bars, standing in for lines of text.
fn draw(width: u32, height: u32, lines: u32) -> Result<Vec<u8>, String> {
    if width == 0 || height == 0 || width > 8000 || height > 8000 {
        return Err(format!("image {width}x{height}: sides must be 1 to 8000 pixels"));
    }
    let mut img = image::GrayImage::from_pixel(width, height, image::Luma([255]));
    let rows = lines.max(1);
    let band = height / (rows * 2 + 1);
    for i in 0..lines {
        let top = band * (i * 2 + 1);
        for y in top..(top + band).min(height) {
            for x in (width / 10)..(width - width / 10) {
                img.put_pixel(x, y, image::Luma([20]));
            }
        }
    }
    let mut out = std::io::Cursor::new(Vec::new());
    img.write_to(&mut out, image::ImageFormat::Png).map_err(|e| e.to_string())?;
    Ok(out.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("glossa-mt-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(d.join("tests")).unwrap();
        d
    }

    fn write(dir: &Path, file: &str, text: &str) {
        std::fs::write(dir.join(file), text).unwrap();
    }

    #[test]
    fn a_js_module_passes_its_cases_offline_and_bad_ones_are_named() {
        let d = temp("js");
        write(
            &d,
            "module.toml",
            "capabilities = [\"translate\"]\n[module]\nid = \"t\"\nname = \"T\"\nversion = \"1.0.0\"\nruntime = \"js\"\nmax_input_chars = 100\n[permissions]\nnetwork = [\"api.test\"]\n[[options]]\nkey = \"api_key\"\ntype = \"secret\"\nlabel = \"Key\"\nrequired = true\n",
        );
        write(
            &d,
            "main.js",
            r#"function translate(req, host) {
                 if (!req.options.api_key) throw new ModuleError("missing_credential", "api_key");
                 var r = host.fetch("https://api.test/t?q=" + encodeURIComponent(req.text));
                 if (!r.ok) throw new ModuleError("http", "status " + r.status);
                 return { text: r.json().out, detected_from: "en" };
               }"#,
        );
        write(
            &d,
            "tests/basic.toml",
            r#"
            options = { api_key = "test-key" }
            [[case]]
            name = "translates"
            op = "translate"
            text = "hi"
            to = "ru"
            [case.expect]
            text = "привет"
            detected = "en"
            [[case.http]]
            url = "https://api.test/t"
            body = '{"out": "привет"}'

            [[case]]
            name = "needs a key"
            op = "translate"
            text = "hi"
            options = { api_key = "" }
            expect = { error = "missing_credential" }

            [[case]]
            name = "wrong expectation"
            op = "translate"
            text = "hi"
            expect = { text = "hello" }
            [[case.http]]
            url = "https://api.test/"
            body = '{"out": "привет"}'

            [[case]]
            name = "unrecorded"
            op = "translate"
            text = "hi"
            "#,
        );
        let r = test_module(&d, &Settings::default());
        let statuses: Vec<_> = r.cases.iter().map(|c| (c.name.as_str(), c.status)).collect();
        assert_eq!(
            statuses,
            [
                ("translates", Status::Pass),
                ("needs a key", Status::Pass),
                ("wrong expectation", Status::Fail),
                ("unrecorded", Status::Fail)
            ],
            "{}",
            r.render()
        );
        assert!(r.cases[2].problems[0].contains("expected the text \"hello\""), "{}", r.render());
        assert!(r.cases[3].problems[0].contains("no recorded response for GET https://api.test/t"), "{}", r.render());
        assert_eq!(r.cases[0].requests, ["GET https://api.test/t"]);
        assert_eq!(r.status, Status::Fail);
        assert_eq!(r.count(Severity::Error), 0, "{}", r.render());
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn action_cases_run_with_recorded_http_and_strings() {
        let d = temp("actions");
        write(
            &d,
            "module.toml",
            r#"capabilities = ["translate"]
[module]
id = "acts"
name = "Acts"
version = "1.0.0"
runtime = "js"
max_input_chars = 100
[permissions]
network = ["api.test"]
[[options]]
key = "api_key"
type = "secret"
label = "Key"
[[options]]
key = "model"
type = "select"
label = "Model"
choices = [{ value = "small" }]
choices_from = "list_models"
[[actions]]
id = "list_models"
label = "Load"
[[actions]]
id = "test"
label = "Test"
needs = ["api_key"]
[[actions]]
id = "untested"
label = "Nobody tests me"
"#,
        );
        std::fs::create_dir_all(d.join("locales")).unwrap();
        write(&d, "locales/ru.toml", "[strings]\nok = \"Готово: {0}\"\n");
        write(
            &d,
            "main.js",
            r#"function translate(req) { return req.text; }
               function list_models(ev, host) {
                 return { choices: { model: host.fetch("https://api.test/models").json() } };
               }
               function test(ev, host) { return { notice: host.t("ok", ev.options.api_key), errors: { ghost: "x" } }; }
               function untested() {}"#,
        );
        write(
            &d,
            "tests/basic.toml",
            r#"
            [[case]]
            name = "translates"
            op = "translate"
            text = "hi"
            expect = { text = "hi" }

            [[case]]
            name = "loads models"
            op = "action"
            action = "list_models"
            [case.expect]
            result = { choices = { model = [{ value = "small" }, { value = "large" }] } }
            [[case.http]]
            url = "https://api.test/models"
            body = '["small", "large"]'

            [[case]]
            name = "tests in Russian"
            op = "action"
            action = "test"
            ui_lang = "ru"
            options = { api_key = "fake" }
            expect = { text = "Готово: fake" }

            [[case]]
            name = "no such action"
            op = "action"
            action = "ghost"
            "#,
        );
        let r = test_module(&d, &Settings { check_only: true, ..Settings::default() });
        let text = r.render();
        assert!(text.contains("action \"ghost\" is not in the manifest"), "{text}");
        assert!(text.contains("action \"untested\" has no case"), "{text}");

        // Without the bad case, the rest runs.
        let fixture = std::fs::read_to_string(d.join("tests/basic.toml")).unwrap();
        write(&d, "tests/basic.toml", fixture.split("[[case]]\n            name = \"no such").next().unwrap());
        let r = test_module(&d, &Settings::default());
        let statuses: Vec<_> = r.cases.iter().map(|c| (c.name.as_str(), c.status)).collect();
        assert_eq!(
            statuses,
            [("translates", Status::Pass), ("loads models", Status::Pass), ("tests in Russian", Status::Fail)],
            "{}",
            r.render()
        );
        assert!(r.cases[2].problems[0].contains("names \"ghost\""), "{}", r.render());
        assert_eq!(r.cases[1].requests, ["GET https://api.test/models"]);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn a_crash_does_not_pass_for_not_found_and_recordings_win() {
        let d = temp("nf");
        write(
            &d,
            "module.toml",
            "capabilities = [\"dictionary\"]\n[module]\nid = \"d\"\nname = \"D\"\nversion = \"1.0.0\"\nruntime = \"js\"\nmax_input_chars = 9\n[permissions]\nnetwork = [\"api.test\"]\n",
        );
        write(
            &d,
            "main.js",
            r#"function dictionary(req, host) {
                 if (req.text === "crash") return undefined.boom;
                 var r = host.fetch("https://api.test/" + req.text).json();
                 if (!r.def) throw new ModuleError("not_found", "no entry for " + req.text);
                 return { headword: req.text, explanations: [{ part_of_speech: "n", definitions: [r.def] }] };
               }"#,
        );
        write(
            &d,
            "tests/basic.toml",
            r#"
            [[case]]
            name = "crash"
            op = "dictionary"
            text = "crash"
            expect = { error = "not_found" }

            [[case]]
            name = "miss"
            op = "dictionary"
            text = "zz"
            expect = { error = "not_found", error_contains = "no entry for zz" }
            [[case.http]]
            url = "https://api.test/zz"
            body = '{}'

            [[case]]
            name = "fresh"
            op = "dictionary"
            text = "lamp"
            expect = { contains = "recorded" }
            [[case.http]]
            url = "https://api.test/lamp"
            body = '{"def": "hand-written"}'
            "#,
        );
        let recording = fixture::recording_path(&d.join("tests/basic.toml"), "fresh");
        std::fs::create_dir_all(recording.parent().unwrap()).unwrap();
        std::fs::write(
            &recording,
            r#"{"exchanges": [{"url": "https://api.test/lamp", "body": "{\"def\": \"recorded\"}"}]}"#,
        )
        .unwrap();

        let r = test_module(&d, &Settings::default());
        let status: Vec<_> = r.cases.iter().map(|c| c.status).collect();
        assert_eq!(status, [Status::Fail, Status::Pass, Status::Pass], "{}", r.render());
        assert!(r.cases[0].problems.iter().any(|p| p.contains("add error_contains")), "{}", r.render());

        // Pinned to the hand-written entries, the recording is ignored.
        let pinned = test_module(&d, &Settings { ignore_recordings: true, ..Settings::default() });
        assert_eq!(pinned.cases[2].status, Status::Fail, "{}", pinned.render());
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn manifest_mistakes_are_reported_without_running() {
        let d = temp("lint");
        // A secret-looking default, put together so this file trips no scanner.
        let value = "q8Zr2Lm4Np6V".repeat(2);
        write(
            &d,
            "module.toml",
            &format!("capabilities = [\"translate\"]\nlanguages = [\"en\", \"klingon\"]\n[module]\nid = \"t\"\nname = \"T\"\nruntime = \"js\"\nentyr = \"main.js\"\n[permissions]\nnetwork = [\"https://api.test/v1\"]\n[[options]]\nkey = \"api_key\"\ntype = \"secret\"\ndefault = \"{value}\"\n"),
        );
        write(&d, "main.js", "function translate(req) { return req.text; }");
        let r = test_module(&d, &Settings::default());
        let text = r.render();
        assert_eq!(r.status, Status::Fail, "{text}");
        assert!(text.contains("unknown key `module.entyr`"), "{text}");
        assert!(text.contains("\"klingon\""), "{text}");
        assert!(text.contains("not a host name"), "{text}");
        assert!(text.contains("secret option \"api_key\" has a default"), "{text}");
        assert!(text.contains("no fixtures"), "{text}");
        assert!(r.load_ms.is_none(), "an invalid module is not run");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn a_module_whose_files_are_not_downloaded_is_skipped_with_the_command() {
        let d = temp("assets");
        write(
            &d,
            "module.toml",
            "capabilities = [\"translate\"]\n[module]\nid = \"needs-model\"\nname = \"Needs a model\"\nversion = \"1.0.0\"\nruntime = \"process\"\ncommand = [\"${ASSETS_DIR}/tool\"]\n[permissions]\nnetwork = [\"dl.test\"]\n[[assets]]\nname = \"tool\"\nurl = \"https://dl.test/tool\"\nsha256 = \"b94d27b9934d3e08a52e52d7da7dabfac484efe37a5380ee9088f7ace2efcde9\"\nsize = 11\nexecutable = true\n",
        );
        write(&d, "tests/basic.toml", "[[case]]\nname = \"hi\"\nop = \"translate\"\ntext = \"hi\"\nto = \"de\"\n");
        let models = d.join("models");
        let s = Settings { models_dir: Some(models.clone()), ..Settings::default() };
        let r = test_module(&d, &s);
        assert_eq!(r.status, Status::Skip, "{r:?}");
        let why = r.skipped.clone().unwrap_or_default();
        assert!(why.contains("tool") && why.contains("glossa-cli module assets download needs-model"), "{why}");
        // The command naming the asset is not reported as a missing program.
        assert!(r.findings.iter().all(|f| f.severity != Severity::Error), "{:?}", r.findings);

        // CI asks for every module to run: then it is a failure.
        let strict = Settings { models_dir: Some(models), require_runtimes: true, ..Settings::default() };
        assert_eq!(test_module(&d, &strict).status, Status::Fail);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn a_placeholder_hash_fails_the_module() {
        let d = temp("placeholder");
        let zeros = "0".repeat(64);
        write(
            &d,
            "module.toml",
            &format!("capabilities = [\"translate\"]\n[module]\nid = \"unreleased\"\nname = \"Unreleased\"\nversion = \"1.0.0\"\nruntime = \"process\"\ncommand = [\"${{ASSETS_DIR}}/tool\"]\n[permissions]\nnetwork = [\"dl.test\"]\n[[assets]]\nname = \"tool\"\nurl = \"https://dl.test/tool\"\nsha256 = \"{zeros}\"\nsize = 11\n"),
        );
        write(&d, "tests/basic.toml", "[[case]]\nname = \"hi\"\nop = \"translate\"\ntext = \"hi\"\nto = \"de\"\n");
        let s = Settings { models_dir: Some(d.join("models")), strict: true, ..Settings::default() };
        for r in [test_module(&d, &s), test_module(&d, &Settings { check_only: true, ..s.clone() })] {
            assert_eq!(r.status, Status::Fail, "{r:?}");
            assert!(
                r.findings.iter().any(|f| f.severity == Severity::Error && f.message.contains("placeholder")),
                "{:?}",
                r.findings
            );
        }
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn modules_are_found_below_a_folder() {
        let d = temp("find");
        for m in ["a/x", "a/y", "b"] {
            std::fs::create_dir_all(d.join(m)).unwrap();
            write(&d.join(m), "module.toml", "");
        }
        std::fs::create_dir_all(d.join("b/inner")).unwrap();
        write(&d.join("b/inner"), "module.toml", "");
        let found: Vec<_> = find_modules(&d).into_iter().map(|p| p.strip_prefix(&d).unwrap().to_owned()).collect();
        assert_eq!(found, [Path::new("a/x"), Path::new("a/y"), Path::new("b")]);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn drawn_images_have_the_asked_size() {
        let png = draw(120, 50, 2).unwrap();
        assert_eq!(glossa_modules::protocol::image_dimensions(&png), Some((120, 50)));
        assert!(draw(0, 10, 1).is_err());
    }
}
