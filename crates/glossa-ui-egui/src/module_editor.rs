//! The module editor window: the module's files on the left, editor tabs in
//! the middle, a preview of its settings page on the right, and a drawer
//! with Problems, Log and Test results at the bottom.
//!
//! The window draws and dispatches. Reading and saving files, checking,
//! reloading and testing are `glossa_engine::module_dev` calls made on a
//! worker thread; their answers come back through a channel and wake the
//! window. Nothing exists until the window is opened, and it goes away with
//! it (closing with unsaved files asks first).

use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, Sender};

use egui::RichText;
use glossa_engine::module_dev::{
    self, ModuleFile, ModuleReport, Problem, ReloadOutcome, Severity, Template, TestStatus,
};
use glossa_modules::log_tap::LogTap;
use glossa_modules::{OptionValues, SettingsPage};

use crate::app::GlossaApp;
use crate::code_editor::{self, EditorState, Lang, Mark};

/// The viewport's name.
pub const VIEWPORT: &str = "glossa-module-editor";

/// The id salt of the preview's settings page, for `module_settings::select_tab`.
pub const PREVIEW_SALT: &str = "module-editor-preview";

/// Files larger than this open read-only: every keystroke in a `TextEdit`
/// costs time in proportion to the text.
const EDIT_LIMIT: usize = 512 << 10;

/// What to open the editor on.
pub enum Target {
    /// A module folder that can be edited where it is.
    Dir(PathBuf),
    /// A new module from a template, with this name.
    New(Template, String),
    /// A bundled or installed module, copied to the user's folder first.
    Copy(String),
}

/// An open file.
struct Buffer {
    path: String,
    text: String,
    /// The text as last read or saved, to tell whether it changed.
    saved: String,
    lang: Lang,
    /// Too large to edit comfortably: shown, searchable, not editable.
    read_only: bool,
    editor: EditorState,
}

impl Buffer {
    fn dirty(&self) -> bool {
        self.text != self.saved
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Drawer {
    Problems,
    Log,
    Tests,
    /// The preview, on a window too narrow for it beside the editor.
    Preview,
}

/// What a worker sends back.
enum Reply {
    /// The folder is ready (copied or created): what to edit.
    Ready(Result<PathBuf, String>),
    Files(Vec<ModuleFile>),
    Opened(String, Result<String, String>),
    /// Files saved (or the error), then the check that followed.
    Saved {
        written: Vec<(String, String)>,
        error: Option<String>,
    },
    Checked {
        problems: Vec<Problem>,
        info: Option<Box<Info>>,
    },
    Reloaded(Result<ReloadOutcome, String>),
    Tested(Box<ModuleReport>),
}

/// What the manifest says, for the title and the preview.
#[derive(Clone, Default)]
struct Info {
    id: String,
    name: String,
    version: String,
    page: SettingsPage,
    /// The module's panels, built from the manifest as last checked.
    panels: Vec<std::sync::Arc<glossa_modules::panels::PanelPage>>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum After {
    Nothing,
    /// Reload when the check finds no errors.
    Reload,
    /// Run the tests once saved.
    Test,
}

pub struct ModuleEditor {
    dir: Option<PathBuf>,
    info: Info,
    files: Vec<ModuleFile>,
    tabs: Vec<Buffer>,
    active: usize,
    problems: Vec<Problem>,
    report: Option<ModuleReport>,
    drawer: Drawer,
    log: Option<LogTap>,
    /// Option values typed into the preview; never saved.
    preview_values: OptionValues,
    /// The panel the preview shows, live: which one (by index into
    /// `info.panels`) and its engine-side state.
    preview_panel: Option<(usize, glossa_engine::Panel)>,
    preview_panel_choice: usize,
    tx: Sender<Reply>,
    rx: Receiver<Reply>,
    /// Workers still running.
    busy: usize,
    status: Option<(String, bool)>,
    after_check: After,
    /// Closing was asked for with unsaved files.
    confirm_close: bool,
    /// Close the window (or quit Glossa, when quitting) once the files are
    /// saved: "Save all" in the confirmation.
    close_after_save: bool,
    /// Glossa is quitting: the confirmation is for quitting, not closing.
    quitting: bool,
    /// The user chose to throw the changes away.
    discarded: bool,
    /// Close the window at the next frame.
    close_now: bool,
    /// Open this file when the folder's files arrive, and jump here.
    pending_open: Vec<String>,
    /// The file to bring forward when it has been read.
    focus_on: Option<String>,
    /// The first check has not come back yet: it shows the first error.
    first_check: bool,
    pending_jump: Option<(String, u32, u32)>,
    /// Run the tests as soon as the module is ready (`--screenshot
    /// module-editor:test`).
    test_on_ready: bool,
}

/// Open the editor on `target`, or bring it forward (a different module
/// replaces the open one when nothing is unsaved).
pub fn open(app: &mut GlossaApp, target: Target) {
    if let Some(ed) = app.module_editor.as_mut() {
        let same = matches!((&target, &ed.dir), (Target::Dir(d), Some(cur)) if d == cur);
        if same || ed.tabs.iter().any(Buffer::dirty) {
            crate::chrome::raise(&app.ctx, crate::chrome::viewport(VIEWPORT));
            return;
        }
    }
    // The preview's actions belonged to the module open before.
    crate::module_actions::reset_pages(|id| id == PREVIEW_SALT);
    let (tx, rx) = mpsc::channel();
    let mut ed = ModuleEditor {
        dir: None,
        info: Info::default(),
        files: Vec::new(),
        tabs: Vec::new(),
        active: 0,
        problems: Vec::new(),
        report: None,
        drawer: Drawer::Problems,
        log: None,
        preview_values: OptionValues::new(),
        preview_panel: None,
        preview_panel_choice: 0,
        tx,
        rx,
        busy: 0,
        status: None,
        after_check: After::Nothing,
        confirm_close: false,
        close_after_save: false,
        quitting: false,
        discarded: false,
        close_now: false,
        pending_open: vec!["module.toml".into()],
        focus_on: None,
        first_check: true,
        pending_jump: None,
        test_on_ready: false,
    };
    let engine = app.engine.clone();
    ed.spawn(&app.ctx, move || {
        Reply::Ready(match target {
            Target::Dir(d) => Ok(d),
            Target::New(t, name) => engine.new_module(t, &name).map_err(|e| e.to_string()),
            Target::Copy(id) => engine.copy_module_to_user(&id).map_err(|e| e.to_string()),
        })
    });
    app.module_editor = Some(ed);
    app.ctx.request_repaint();
}

impl ModuleEditor {
    /// Run the module's tests once it is open and checked.
    pub fn test_when_ready(&mut self) {
        self.test_on_ready = true;
    }

    fn spawn(&mut self, ctx: &egui::Context, job: impl FnOnce() -> Reply + Send + 'static) {
        self.busy += 1;
        let tx = self.tx.clone();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let _ = tx.send(job());
            ctx.request_repaint();
        });
    }

    fn list_files(&mut self, ctx: &egui::Context) {
        let Some(dir) = self.dir.clone() else { return };
        self.spawn(ctx, move || Reply::Files(module_dev::module_files(&dir)));
    }

    fn open_file(&mut self, ctx: &egui::Context, path: &str) {
        if let Some(i) = self.tabs.iter().position(|b| b.path == path) {
            self.active = i;
            return;
        }
        let Some(dir) = self.dir.clone() else { return };
        let path = path.to_owned();
        self.focus_on = Some(path.clone());
        self.spawn(ctx, move || {
            let r = module_dev::read_module_file(&dir, &path).map_err(|e| e.to_string());
            Reply::Opened(path, r)
        });
    }

    fn check(&mut self, app: &GlossaApp, then: After) {
        let Some(dir) = self.dir.clone() else { return };
        self.after_check = then;
        let engine = app.engine.clone();
        let lang = app.locale.code.clone();
        self.spawn(&app.ctx, move || {
            let problems = engine.check_module(&dir);
            let info = manifest_info(&dir, &lang).map(Box::new);
            Reply::Checked { problems, info }
        });
    }

    /// Save every changed file, then check (and reload or test after).
    fn save(&mut self, app: &GlossaApp, then: After) {
        let Some(dir) = self.dir.clone() else { return };
        let dirty: Vec<(String, String)> =
            self.tabs.iter().filter(|b| b.dirty()).map(|b| (b.path.clone(), b.text.clone())).collect();
        if dirty.is_empty() {
            if then == After::Test {
                self.test(app);
            } else {
                self.check(app, then);
            }
            return;
        }
        self.after_check = then;
        self.spawn(&app.ctx, move || {
            // Each file as it was handed over, once it is on disk; the
            // buffers count as saved only from what really was written.
            let mut written = Vec::new();
            for (p, t) in dirty {
                if let Err(e) = module_dev::save_module_file(&dir, &p, &t) {
                    return Reply::Saved { written, error: Some(format!("{p}: {e}")) };
                }
                written.push((p, t));
            }
            Reply::Saved { written, error: None }
        });
    }

    fn reload(&mut self, app: &GlossaApp) {
        let Some(dir) = self.dir.clone() else { return };
        let engine = app.engine.clone();
        self.spawn(&app.ctx, move || Reply::Reloaded(engine.reload_module_approved(&dir).map_err(|e| e.to_string())));
    }

    fn test(&mut self, app: &GlossaApp) {
        let Some(dir) = self.dir.clone() else { return };
        let engine = app.engine.clone();
        self.drawer = Drawer::Tests;
        self.spawn(&app.ctx, move || Reply::Tested(Box::new(engine.test_module(&dir))));
    }

    /// Act on what the workers sent back.
    fn poll(&mut self, app: &mut GlossaApp) {
        while let Ok(reply) = self.rx.try_recv() {
            self.busy = self.busy.saturating_sub(1);
            match reply {
                Reply::Ready(Ok(dir)) => {
                    self.dir = Some(dir);
                    self.list_files(&app.ctx);
                    self.check(app, After::Nothing);
                }
                Reply::Ready(Err(e)) => self.status = Some((e, true)),
                Reply::Files(files) => {
                    self.files = files;
                    // The manifest and the script, the script in front.
                    let mut first = std::mem::take(&mut self.pending_open);
                    if !first.is_empty() {
                        // The entry script: `main.*` beside the manifest, or a classic `Service.js`.
                        let script = self.files.iter().find(|f| {
                            f.text && !f.path.contains('/') && (f.path.starts_with("main.") || f.path == "Service.js")
                        });
                        first.extend(script.map(|f| f.path.clone()));
                    }
                    for p in first {
                        if self.files.iter().any(|f| f.path == p && f.text) {
                            self.open_file(&app.ctx, &p);
                        }
                    }
                    if let Some((file, line, col)) = self.pending_jump.take() {
                        self.go_to(&app.ctx, &file, line, col);
                    }
                }
                Reply::Opened(path, Ok(text)) => {
                    if !self.tabs.iter().any(|b| b.path == path) {
                        let lang = Lang::for_path(&path);
                        let front = self.focus_on.as_deref() == Some(path.as_str()) || self.tabs.is_empty();
                        self.tabs.push(Buffer {
                            read_only: text.len() > EDIT_LIMIT,
                            saved: text.clone(),
                            text,
                            lang,
                            path,
                            editor: EditorState::default(),
                        });
                        if front {
                            self.active = self.tabs.len() - 1;
                        }
                    }
                    self.apply_pending_jump();
                }
                Reply::Opened(_, Err(e)) => self.status = Some((e, true)),
                Reply::Saved { written, error } => {
                    mark_saved(&mut self.tabs, &written);
                    if let Some(e) = error {
                        // Nothing is closed or quit over files that are not
                        // on disk.
                        self.status = Some((e, true));
                        self.after_check = After::Nothing;
                        if std::mem::take(&mut self.close_after_save) {
                            // Ask again, with the error in view.
                            self.confirm_close = true;
                        }
                        continue;
                    }
                    self.status = Some((app.t("editor.saved"), false));
                    if std::mem::take(&mut self.close_after_save) && !self.dirty() {
                        if self.quitting {
                            app.quit();
                        } else {
                            self.close_now = true;
                        }
                        continue;
                    }
                    self.list_files(&app.ctx);
                    if self.after_check == After::Test {
                        self.after_check = After::Nothing;
                        self.test(app);
                    } else {
                        let then = self.after_check;
                        self.check(app, then);
                    }
                }
                Reply::Checked { problems, info } => {
                    let errors = problems.iter().any(|p| p.severity == Severity::Error);
                    let first = problems.iter().find(|p| p.severity == Severity::Error && p.line > 0).cloned();
                    self.problems = problems;
                    if std::mem::take(&mut self.first_check) {
                        if let Some(p) = first {
                            self.go_to(&app.ctx, &p.file, p.line, p.column);
                        }
                    }
                    if let Some(info) = info {
                        self.adopt_info(app, *info);
                    }
                    if errors && self.drawer != Drawer::Tests {
                        self.drawer = Drawer::Problems;
                    }
                    match std::mem::replace(&mut self.after_check, After::Nothing) {
                        After::Reload if errors => self.status = Some((app.t("editor.not_reloaded"), true)),
                        After::Reload => self.reload(app),
                        _ => {}
                    }
                    if std::mem::take(&mut self.test_on_ready) {
                        self.test(app);
                    }
                }
                Reply::Reloaded(r) => {
                    self.status = Some(match r {
                        Ok(ReloadOutcome::Reloaded { id }) => (app.locale.fmt("editor.reloaded", &id), false),
                        Ok(ReloadOutcome::NeedsRestart { .. }) => (app.t("editor.restart_needed"), false),
                        Ok(ReloadOutcome::Disabled { id }) => (app.locale.fmt("editor.disabled", &id), true),
                        Err(e) => (e, true),
                    });
                }
                Reply::Tested(report) => {
                    self.report = Some(*report);
                    self.drawer = Drawer::Tests;
                }
            }
        }
    }

    fn adopt_info(&mut self, app: &GlossaApp, info: Info) {
        if self.log.as_ref().is_none_or(|t| t.module() != info.id) {
            self.log = Some(app.engine.module_log(&info.id));
        }
        if self.info.id != info.id {
            self.preview_values = app.engine.config().services.options.get(&info.id).cloned().unwrap_or_default();
            self.preview_panel_choice = 0;
        }
        // The manifest may have changed the panel: draw it afresh.
        self.preview_panel = None;
        self.info = info;
    }

    fn apply_pending_jump(&mut self) {
        let Some((file, line, col)) = self.pending_jump.clone() else { return };
        if let Some(i) = self.tabs.iter().position(|b| b.path == file) {
            self.active = i;
            self.tabs[i].editor.jump_to(line, col);
            self.pending_jump = None;
        }
    }

    /// Show `file` at `line:col`, opening it if need be.
    fn go_to(&mut self, ctx: &egui::Context, file: &str, line: u32, col: u32) {
        let Some(f) = self.resolve(file) else {
            // The folder's files are not listed yet: go there once they are.
            if self.files.is_empty() {
                self.pending_jump = Some((file.to_owned(), line, col));
            }
            return;
        };
        self.pending_jump = Some((f.clone(), line, col));
        if self.tabs.iter().any(|b| b.path == f) {
            self.apply_pending_jump();
        } else {
            self.open_file(ctx, &f);
        }
    }

    /// A file named in a message, as a path in the folder: exact, or by its
    /// file name (a script names itself `main.js` wherever it lives).
    fn resolve(&self, file: &str) -> Option<String> {
        if file.is_empty() {
            return None;
        }
        self.files
            .iter()
            .find(|f| f.path == file)
            .or_else(|| self.files.iter().find(|f| f.path.rsplit('/').next() == Some(file)))
            .map(|f| f.path.clone())
    }

    /// Every problem worth listing: the check's, and the places failed test
    /// cases name.
    fn all_problems(&self) -> Vec<Problem> {
        let mut all = self.problems.clone();
        if let Some(r) = &self.report {
            for c in r.cases.iter().filter(|c| c.status == TestStatus::Fail) {
                for p in &c.problems {
                    if let Some((file, line, column)) = module_dev::find_place(p) {
                        if let Some(path) = self.resolve(&file) {
                            all.push(Problem {
                                file: path,
                                line,
                                column,
                                severity: Severity::Error,
                                message: format!("{}: {p}", c.name),
                            });
                        }
                    }
                }
            }
        }
        all
    }

    fn marks_for(&self, path: &str) -> Vec<Mark> {
        self.all_problems()
            .into_iter()
            .filter(|p| p.file == path && p.line > 0)
            .map(|p| Mark { line: p.line, column: p.column, message: p.message, error: p.severity == Severity::Error })
            .collect()
    }

    fn dirty(&self) -> bool {
        self.tabs.iter().any(Buffer::dirty)
    }

    /// Glossa is about to quit. With unsaved files (not thrown away), show
    /// the confirmation in the editor and return true: the quit waits for
    /// the answer.
    pub fn hold_quit(&mut self) -> bool {
        if !self.dirty() || self.discarded {
            return false;
        }
        self.quitting = true;
        self.confirm_close = true;
        true
    }
}

/// After a save: each buffer counts as saved up to the text that was written
/// for it. One edited since (or not written at all) stays unsaved.
fn mark_saved(tabs: &mut [Buffer], written: &[(String, String)]) {
    for (path, text) in written {
        if let Some(b) = tabs.iter_mut().find(|b| &b.path == path) {
            b.saved = text.clone();
        }
    }
}

/// The id, name, version and settings page (in `lang`, with the module's
/// `locales/`) from the folder's `module.toml`, if it loads.
fn manifest_info(dir: &std::path::Path, lang: &str) -> Option<Info> {
    let m = glossa_modules::Manifest::load(&dir.join("module.toml")).ok()?;
    let page = m.settings_page(lang);
    let panels = glossa_modules::panels::pages(&m, lang).into_iter().map(std::sync::Arc::new).collect();
    Some(Info { id: m.module.id, name: m.module.name, version: m.module.version, page, panels })
}

pub fn show(app: &mut GlossaApp, ctx: &egui::Context) {
    let Some(mut ed) = app.module_editor.take() else { return };
    ed.poll(app);
    let title = if ed.info.name.is_empty() {
        app.t("editor.title")
    } else {
        app.locale.fmt("editor.title_named", &ed.info.name)
    };
    let builder =
        crate::chrome::builder(app, &title).with_inner_size([1180.0, 760.0]).with_min_inner_size([640.0, 420.0]);
    let mut open = true;
    let custom = crate::chrome::custom(&app.cfg);
    ctx.show_viewport_immediate(crate::chrome::viewport(VIEWPORT), builder, |ui, _| {
        if ui.input(|i| i.viewport().close_requested()) {
            if ed.dirty() && !ed.discarded {
                ui.ctx().send_viewport_cmd(egui::ViewportCommand::CancelClose);
                ed.confirm_close = true;
            } else {
                open = false;
            }
        }
        if ed.close_now {
            open = false;
        }
        crate::chrome::frame(ui, &app.look, custom, &crate::chrome::Caption::new(&title));
        body(app, ui, &mut ed, &mut open);
    });
    app.child_backdrop(&title, false);
    if open {
        app.module_editor = Some(ed);
    }
}

fn body(app: &mut GlossaApp, ui: &mut egui::Ui, ed: &mut ModuleEditor, open: &mut bool) {
    let look = app.look.clone();
    let cmd = |k: egui::Key| egui::KeyboardShortcut::new(egui::Modifiers::COMMAND, k);
    if ui.input_mut(|i| i.consume_shortcut(&cmd(egui::Key::S))) {
        ed.save(app, After::Reload);
    }
    if ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::F5)) {
        ed.save(app, After::Test);
    }

    if ed.confirm_close {
        egui::Panel::top("editor-confirm").show(ui, |ui| {
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                ui.label(RichText::new(app.t("editor.unsaved")).strong());
                if ui.button(app.t("editor.save_all")).clicked() {
                    // Closes (or quits) once every file is on disk.
                    ed.close_after_save = true;
                    ed.save(app, After::Nothing);
                    ed.confirm_close = false;
                }
                if ui.button(app.t("editor.discard")).clicked() {
                    ed.discarded = true;
                    if ed.quitting {
                        app.quit();
                    } else {
                        *open = false;
                    }
                }
                if ui.button(app.t("editor.keep_editing")).clicked() {
                    ed.confirm_close = false;
                    if std::mem::take(&mut ed.quitting) {
                        app.cancel_quit();
                    }
                }
            });
            ui.add_space(6.0);
        });
    }

    toolbar(app, ui, ed);

    let narrow = ui.available_width() < 900.0;
    if narrow && ed.drawer != Drawer::Preview {
        // Fine; the drawer offers the preview as a tab.
    } else if !narrow && ed.drawer == Drawer::Preview {
        ed.drawer = Drawer::Problems;
    }
    egui::Panel::bottom("editor-drawer").resizable(true).default_size(190.0).min_size(90.0).show(ui, |ui| {
        drawer(app, ui, ed, narrow);
    });
    egui::Panel::left("editor-files").resizable(true).default_size(180.0).min_size(120.0).show(ui, |ui| {
        files(app, ui, ed);
    });
    if !narrow {
        egui::Panel::right("editor-preview").resizable(true).default_size(320.0).min_size(220.0).show(ui, |ui| {
            preview(app, ui, ed);
        });
    }
    egui::CentralPanel::default().show(ui, |ui| {
        editor(app, ui, ed, &look);
    });
}

fn toolbar(app: &mut GlossaApp, ui: &mut egui::Ui, ed: &mut ModuleEditor) {
    let look = app.look.clone();
    egui::Panel::top("editor-toolbar").show(ui, |ui| {
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            let ready = ed.dir.is_some();
            let save_tip = app.locale.fmt("editor.save_tip", &crate::widgets::shortcut_text("Ctrl+S"));
            if ui.add_enabled(ready, egui::Button::new(app.t("editor.save"))).on_hover_text(save_tip).clicked() {
                ed.save(app, After::Reload);
            }
            if ui.add_enabled(ready, egui::Button::new(app.t("editor.check"))).clicked() {
                ed.save(app, After::Nothing);
            }
            if ui.add_enabled(ready, egui::Button::new(app.t("editor.reload"))).clicked() {
                ed.save(app, After::Reload);
            }
            let test_tip = app.t("editor.test_tip");
            if ui.add_enabled(ready, egui::Button::new(app.t("editor.test"))).on_hover_text(test_tip).clicked() {
                ed.save(app, After::Test);
            }
            if ui.add_enabled(ready, egui::Button::new(app.t("editor.open_folder"))).clicked() {
                if let Some(d) = &ed.dir {
                    let _ = crate::app::open_with_system(&d.to_string_lossy());
                }
            }
            if ed.busy > 0 {
                ui.spinner();
                ui.label(RichText::new(app.t("editor.working")).color(look.weak_text()));
            } else if let Some((text, err)) = &ed.status {
                ui.label(RichText::new(text).color(if *err { look.error() } else { look.weak_text() }));
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if !ed.info.id.is_empty() {
                    ui.label(
                        RichText::new(format!("{} {}", ed.info.id, ed.info.version))
                            .color(look.weak_text())
                            .monospace(),
                    );
                }
            });
        });
        ui.add_space(4.0);
    });
}

fn files(app: &mut GlossaApp, ui: &mut egui::Ui, ed: &mut ModuleEditor) {
    let look = app.look.clone();
    ui.add_space(4.0);
    ui.label(RichText::new(app.t("editor.files")).strong().color(look.weak_text()));
    egui::ScrollArea::vertical().id_salt("editor-files-scroll").auto_shrink([false, false]).show(ui, |ui| {
        let mut open = None;
        let active = ed.tabs.get(ed.active).map(|b| b.path.clone());
        let mut folder = "";
        for f in &ed.files {
            let depth = f.path.matches('/').count();
            let name = f.path.rsplit('/').next().unwrap_or(&f.path);
            // A folder's name above its first file.
            let parent = f.path.rsplit_once('/').map_or("", |(p, _)| p);
            if parent != folder && !parent.is_empty() {
                let shown = parent.rsplit('/').next().unwrap_or(parent);
                ui.horizontal(|ui| {
                    ui.add_space(depth.saturating_sub(1) as f32 * 12.0 + 4.0);
                    ui.label(RichText::new(format!("{shown}/")).color(look.weak_text()));
                });
            }
            folder = parent;
            ui.horizontal(|ui| {
                ui.add_space(depth as f32 * 12.0);
                if f.text {
                    let selected = active.as_deref() == Some(f.path.as_str());
                    let dirty = ed.tabs.iter().any(|b| b.path == f.path && b.dirty());
                    let label = if dirty { format!("{name} *") } else { name.to_owned() };
                    let has_error = ed.problems.iter().any(|p| p.file == f.path && p.severity == Severity::Error);
                    let text = if has_error { RichText::new(label).color(look.error()) } else { RichText::new(label) };
                    if ui.selectable_label(selected, text).on_hover_text(&f.path).clicked() {
                        open = Some(f.path.clone());
                    }
                } else {
                    ui.label(RichText::new(name).color(look.weak_text())).on_hover_text(app.t("editor.binary"));
                }
            });
        }
        if let Some(p) = open {
            ed.open_file(&app.ctx, &p);
        }
    });
}

fn editor(app: &mut GlossaApp, ui: &mut egui::Ui, ed: &mut ModuleEditor, look: &crate::style::Look) {
    if ed.tabs.is_empty() {
        ui.centered_and_justified(|ui| {
            let text = if let Some((e, true)) = &ed.status { e.clone() } else { app.t("editor.working") };
            ui.label(RichText::new(text).color(look.weak_text()));
        });
        return;
    }
    // Tabs.
    let mut close = None;
    ui.horizontal_wrapped(|ui| {
        for (i, b) in ed.tabs.iter().enumerate() {
            let name = b.path.rsplit('/').next().unwrap_or(&b.path);
            let label = if b.dirty() { format!("{name} *") } else { name.to_owned() };
            let r = ui.selectable_label(i == ed.active, label).on_hover_text(&b.path);
            if r.clicked() {
                ed.active = i;
            }
            if r.middle_clicked() && !b.dirty() {
                close = Some(i);
            }
            if i == ed.active
                && !b.dirty()
                && ed.tabs.len() > 1
                && crate::icons::sized_button(
                    ui,
                    look,
                    crate::icons::Icon::Close,
                    &app.t("editor.close_tab"),
                    14.0,
                    true,
                )
                .clicked()
            {
                close = Some(i);
            }
        }
    });
    if let Some(i) = close {
        ed.tabs.remove(i);
        ed.active = ed.active.min(ed.tabs.len().saturating_sub(1));
    }
    let labels = code_editor::Labels {
        find: app.t("editor.find"),
        next: app.t("editor.next"),
        previous: app.t("editor.previous"),
        go_to_line: app.t("editor.go_to_line"),
        matches: app.t("editor.matches"),
        no_matches: app.t("editor.no_matches"),
    };
    let marks = ed.tabs.get(ed.active).map(|b| ed.marks_for(&b.path)).unwrap_or_default();
    let Some(b) = ed.tabs.get_mut(ed.active) else { return };
    let (line, col) = b.editor.cursor;
    let place = if line > 0 {
        app.locale.fmt_args("editor.line_col", &[&line.to_string(), &col.to_string()])
    } else {
        String::new()
    };
    let read_only = if b.read_only { format!("{}    ", app.t("editor.read_only")) } else { String::new() };
    let status = format!("{read_only}{place}    {}", crate::widgets::shortcut_text(&app.t("editor.keys_hint")));
    egui::Panel::bottom("editor-status").show(ui, |ui| {
        ui.label(RichText::new(status).small().color(look.weak_text()));
    });
    let path = b.path.clone();
    code_editor::show(ui, look, &labels, &mut b.editor, &path, b.lang, &mut b.text, b.read_only, &marks);
}

fn drawer(app: &mut GlossaApp, ui: &mut egui::Ui, ed: &mut ModuleEditor, narrow: bool) {
    let look = app.look.clone();
    let problems = ed.all_problems();
    ui.add_space(4.0);
    ui.horizontal(|ui| {
        let errors = problems.iter().filter(|p| p.severity == Severity::Error).count();
        let tabs = [
            (Drawer::Problems, app.locale.fmt("editor.problems", &errors.to_string())),
            (Drawer::Log, app.t("editor.log")),
            (Drawer::Tests, app.t("editor.tests")),
        ];
        for (d, label) in tabs {
            if ui.selectable_label(ed.drawer == d, label).clicked() {
                ed.drawer = d;
            }
        }
        if narrow && ui.selectable_label(ed.drawer == Drawer::Preview, app.t("editor.preview")).clicked() {
            ed.drawer = Drawer::Preview;
        }
        if ed.drawer == Drawer::Log {
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.small_button(app.t("editor.clear")).clicked() {
                    if let Some(t) = &ed.log {
                        t.clear();
                    }
                }
            });
        }
    });
    ui.separator();
    let mut jump = None;
    egui::ScrollArea::vertical()
        .id_salt(("editor-drawer", ed.drawer as u8))
        .auto_shrink([false, false])
        .stick_to_bottom(ed.drawer == Drawer::Log)
        .show(ui, |ui| match ed.drawer {
            Drawer::Problems => {
                if problems.is_empty() {
                    ui.label(RichText::new(app.t("editor.no_problems")).color(look.weak_text()));
                }
                for p in &problems {
                    ui.horizontal_wrapped(|ui| {
                        let colour = match p.severity {
                            Severity::Error => look.error(),
                            _ => look.weak_text(),
                        };
                        let word = match p.severity {
                            Severity::Error => app.t("editor.error"),
                            Severity::Warning => app.t("editor.warning"),
                            Severity::Note => app.t("editor.note"),
                        };
                        ui.label(RichText::new(word).color(colour).small().strong());
                        let place = p.place();
                        if !place.is_empty() && ui.link(RichText::new(&place).monospace()).clicked() {
                            jump = Some((p.file.clone(), p.line, p.column));
                        }
                        ui.label(&p.message);
                    });
                }
            }
            Drawer::Log => match &ed.log {
                Some(tap) if tap.generation() > 0 || !tap.lines().is_empty() => {
                    for l in tap.lines() {
                        let colour = match l.level.as_str() {
                            "error" | "stderr" => look.error(),
                            "warn" => look.accent(),
                            "debug" | "trace" => look.weak_text(),
                            _ => look.text(),
                        };
                        ui.label(RichText::new(format!("{:<6} {}", l.level, l.message)).monospace().color(colour));
                    }
                }
                _ => {
                    ui.label(RichText::new(app.t("editor.log_empty")).color(look.weak_text()));
                }
            },
            Drawer::Tests => tests(app, ui, ed, &mut jump),
            Drawer::Preview => preview(app, ui, ed),
        });
    if let Some((file, line, col)) = jump {
        ed.go_to(&app.ctx, &file, line, col);
    }
}

fn tests(app: &GlossaApp, ui: &mut egui::Ui, ed: &ModuleEditor, jump: &mut Option<(String, u32, u32)>) {
    let look = &app.look;
    let Some(r) = &ed.report else {
        ui.label(RichText::new(app.t("editor.tests_none")).color(look.weak_text()));
        return;
    };
    let count = |s: TestStatus| r.cases.iter().filter(|c| c.status == s).count().to_string();
    ui.label(
        RichText::new(app.locale.fmt_args(
            "editor.tests_summary",
            &[&count(TestStatus::Pass), &count(TestStatus::Fail), &count(TestStatus::Skip)],
        ))
        .strong(),
    );
    if let Some(why) = &r.skipped {
        ui.label(RichText::new(why).color(look.weak_text()));
    }
    for f in r.findings.iter().filter(|f| f.severity == Severity::Error) {
        ui.label(RichText::new(&f.message).color(look.error()));
    }
    for c in &r.cases {
        let (word, colour) = match c.status {
            TestStatus::Pass => (app.t("editor.pass"), look.weak_text()),
            TestStatus::Fail => (app.t("editor.fail"), look.error()),
            TestStatus::Skip => (app.t("editor.skip"), look.weak_text()),
        };
        ui.horizontal_wrapped(|ui| {
            ui.label(RichText::new(word).color(colour).strong().small());
            ui.label(RichText::new(&c.name).strong());
            ui.label(RichText::new(format!("{}, {} ms", c.file, c.ms)).small().color(look.weak_text()));
            if let Some(res) = &c.result {
                ui.label(RichText::new(res).small().monospace());
            }
        });
        for p in &c.problems {
            ui.horizontal_wrapped(|ui| {
                ui.add_space(18.0);
                if let Some((file, line, col)) = module_dev::find_place(p) {
                    if ui.link(RichText::new(format!("{file}:{line}")).monospace()).clicked() {
                        *jump = Some((file, line, col));
                    }
                }
                ui.label(RichText::new(p).color(look.error()));
            });
        }
        for req in &c.requests {
            ui.horizontal(|ui| {
                ui.add_space(18.0);
                ui.label(RichText::new(req).small().monospace().color(look.weak_text()));
            });
        }
    }
}

fn preview(app: &mut GlossaApp, ui: &mut egui::Ui, ed: &mut ModuleEditor) {
    let look = app.look.clone();
    ui.add_space(4.0);
    ui.label(RichText::new(format!("{}: {}", app.t("editor.preview"), app.t("editor.preview_settings"))).strong());
    ui.label(RichText::new(app.t("editor.preview_note")).small().color(look.weak_text()));
    ui.separator();
    egui::ScrollArea::vertical().id_salt("editor-preview-scroll").auto_shrink([false, false]).show(ui, |ui| {
        let page = &ed.info.page;
        if page.options.is_empty() {
            ui.label(RichText::new(app.t("editor.preview_none")).color(look.weak_text()));
        } else {
            if let Some(d) = &page.description {
                ui.label(RichText::new(d).small());
            }
            // The renderer the Services page uses, on the page as last
            // checked, in the interface language.
            // Its actions run in the loaded module (as last reloaded) with
            // the values typed here.
            let run = crate::module_actions::engine_runner(app, &ed.info.id);
            crate::module_settings::show_with(
                ui,
                &look,
                &app.locale,
                page,
                &mut ed.preview_values,
                false,
                PREVIEW_SALT,
                Some(&run),
            );
        }
        preview_panels(app, ui, ed);
    });
}

/// The module's panels, drawn by the renderer the panel windows use, one
/// at a time. Their actions run in the module as last reloaded; nothing
/// typed here is kept.
fn preview_panels(app: &mut GlossaApp, ui: &mut egui::Ui, ed: &mut ModuleEditor) {
    let look = app.look.clone();
    if ed.info.panels.is_empty() {
        return;
    }
    ui.add_space(12.0);
    ui.separator();
    ui.label(RichText::new(format!("{}: {}", app.t("editor.preview"), app.t("editor.preview_panel"))).strong());
    ui.label(RichText::new(app.t("editor.preview_panel_note")).small().color(look.weak_text()));
    let choice = ed.preview_panel_choice.min(ed.info.panels.len() - 1);
    if ed.info.panels.len() > 1 {
        ui.horizontal_wrapped(|ui| {
            for (i, p) in ed.info.panels.iter().enumerate() {
                if ui.selectable_label(i == choice, &p.title).clicked() {
                    ed.preview_panel_choice = i;
                }
            }
        });
    }
    let choice = ed.preview_panel_choice.min(ed.info.panels.len() - 1);
    if ed.preview_panel.as_ref().is_none_or(|(i, _)| *i != choice) {
        // The pictures of the panel shown before go with it.
        crate::module_panel::forget(&app.ctx, PREVIEW_SALT);
        let ctx = app.ctx.clone();
        let panel = app
            .engine
            .preview_panel(ed.info.panels[choice].clone(), std::sync::Arc::new(move || ctx.request_repaint()));
        ed.preview_panel = Some((choice, panel));
    }
    let Some((_, panel)) = ed.preview_panel.as_mut() else { return };
    if let Some(wait) = panel.poll() {
        ui.ctx().request_repaint_after(wait);
    }
    ui.add_space(4.0);
    let outs = crate::widgets::group(ui, &look, &panel.page().window_title.clone(), |ui| {
        crate::module_panel::draw(ui, &look, &app.locale, panel, PREVIEW_SALT)
    });
    let effects = panel.take_effects();
    crate::module_panel::carry_out(app, outs, effects);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn buffer(path: &str, saved: &str, text: &str) -> Buffer {
        Buffer {
            path: path.into(),
            text: text.into(),
            saved: saved.into(),
            lang: Lang::Plain,
            read_only: false,
            editor: EditorState::default(),
        }
    }

    #[test]
    fn only_what_was_written_counts_as_saved() {
        let mut tabs = vec![buffer("a.js", "old", "new"), buffer("b.js", "old", "new"), buffer("c.js", "x", "y")];
        // a.js was written; b.js was written, then edited again before the
        // answer came; c.js failed and was never written.
        tabs[1].text = "newer".into();
        mark_saved(&mut tabs, &[("a.js".into(), "new".into()), ("b.js".into(), "new".into())]);
        assert!(!tabs[0].dirty());
        assert!(tabs[1].dirty(), "edited after the write");
        assert_eq!(tabs[1].saved, "new");
        assert!(tabs[2].dirty(), "a failed write leaves the buffer unsaved");
    }
}
