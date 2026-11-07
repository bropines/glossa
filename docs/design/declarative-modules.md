# RFC: declarative modules and an in-app module editor

Status: **proposal**, not implemented. Nothing here changes Glossa until
the phases in [the plan](#7-plan) are built and merged.

Two linked features:

- **A. A declarative module format.** A module describes its metadata,
  settings, actions and a small UI (inputs, selects, toggles, buttons,
  result views, a panel of its own) in its manifest. Logic is attached in
  any runtime Glossa already has. Glossa draws the UI natively in egui, in
  its own theme.
- **B. A module editor inside Glossa**, to write and change such modules:
  syntax highlighting, the manifest and a live preview side by side,
  reload on save, and a test run against the module test harness.

The short version of the recommendation:

1. Extend `module.toml` rather than invent a second file: richer
   `[[options]]`, new `[[actions]]`, and an optional `[ui]` with a settings
   layout and panels. Everything is additive, so `api` stays 1 and older
   Glossa builds still load the module as a plain service.
2. Logic is a new protocol method, `action`, that every runtime can
   implement. The embedded language is the one Glossa already ships:
   **JavaScript on boa**. Adding Lua, Rhai or WASM costs size and splits
   the ecosystem for no feature the users asked for.
3. The editor is egui's own `TextEdit` with a small hand-written
   highlighter for JavaScript and TOML, a line-number gutter and error
   underlines, in a window of its own. No syntect, no tree-sitter.

Contents:

1. [What exists today](#1-what-exists-today)
2. [Prior art](#2-prior-art)
3. [The format](#3-the-format)
4. [The scripting language](#4-the-scripting-language)
5. [The editor](#5-the-editor)
6. [Security](#6-security)
7. [Plan](#7-plan)
8. [Open questions](#8-open-questions)

---

## 1. What exists today

### The manifest

A module is a folder with a `module.toml`
(`crates/glossa-modules/src/manifest.rs`, documented in
[../modules/README.md](../modules/README.md#moduletoml)):

```toml
capabilities = ["translate"]           # translate, detect_language, dictionary, listen, ocr
languages = ["en", "ru"]               # omit for all

[module]
id = "my-translator"
name = "My Translator"
version = "1.0.0"
api = 1                                # module protocol major version
runtime = "js"                         # js | python | java | process | native
entry = "main.js"

[language_map]
"zh-Hans" = "zh"

[permissions]
network = ["api.example.com"]

[[options]]
key = "api_key"
type = "secret"                        # text | secret | url | bool | number | select | multiline
label = "API key"
required = true
```

Two facts matter for this RFC:

- `Manifest` has no `deny_unknown_fields`, so **unknown tables and keys are
  ignored**. A module that adds `[ui]` today loads fine in every existing
  build; the UI simply does not appear.
- `validate()` refuses a module whose `api` is not exactly
  `PROTOCOL_VERSION` (1). Bumping the version would break every existing
  module unless the check becomes a range. That is a strong reason to keep
  this work additive.

### Options: the half that is already there

`[[options]]` is already a small declarative form language. Each entry is
an `OptionSpec` (`crates/glossa-modules/src/provider.rs`): `key`, `label`,
`type`, `default`, `help`, `required`, `choices`, `min`, `max`,
`placeholder`. The Services page in Options draws them with
`option_field()` (`crates/glossa-ui-egui/src/options.rs`):

| `type` | widget today |
|---|---|
| `bool` | checkbox |
| `number` | `DragValue`, clamped to `min..=max` |
| `select` | Glossa's `dropdown::ComboBox` over `choices` |
| `multiline` | multi-line `TextEdit` |
| `text`, `url`, `secret` | single-line `TextEdit` (`secret` masked, with the page's "show secrets" switch) |

Values live in `config.toml` under `[services.options.<id>]`; secrets go to
the system keychain when `secret_storage = "system"`; every request carries
them as `options`. The same `OptionSpec` describes built-in providers' and
modules' settings, and the classic and pot-app runtimes translate their own
declarations into it (pot's `needs` list in `runtime/js/pot.rs`, a classic
script's declared options in `legacy.rs`).

What is missing for feature A:

| missing | today |
|---|---|
| layout | options are listed in declaration order, one per row |
| conditions | no "show this only when that is set" |
| validation beyond `required` | none; no patterns, lengths or messages |
| buttons that run module code | none for modules. Built-in AI endpoints have "Load models" and "Test", hard-coded in `ai_tools()` and backed by `Provider::list_models`, which no module runtime implements |
| dynamic choices | `choices` is static |
| a module's own window or panel | none |
| translated module strings | labels are whatever the manifest says |

### The runtimes

All five speak the same protocol; they differ in transport.

| runtime | where it runs | how a call travels | idle cost |
|---|---|---|---|
| `js` | in-process, boa 0.22 | `translate(req, host)`; host calls are synchronous JS functions over one native `__host_call` | none: a fresh `Context` per call |
| `python` | child process (system Python 3.8+) | JSON lines over stdio | none: started on first call, stopped after `idle_timeout_secs` (300) |
| `java` | child process (Java 17+) | same | same |
| `process` | any program | same | same |
| `native` | shared library in-process, behind `[modules] native` | `call(method, params_json)` through the C ABI (`abi.rs`, `glossa_module.h`) | the library |

Plus two compatibility dialects on the same boa engine: classic QTranslate
`Service.js` (`runtime = "qtranslate"`) and pot-app plugins (`"pot"`).

The JS sandbox (`runtime/js/engine.rs`): no I/O except the host; loop limit
50 million iterations; recursion limit 400; Glossa's call timeout. No heap
limit.

### The wire and the version

`glossa-module-sdk::wire` holds the JSON shapes: per-method params
(`TranslateParams`, `DictionaryParams`, `RecognizeParams`…), results, the
`Reply` envelope, `ModuleError` with an `ErrorKind`, and the host-call types
(`HttpRequest`, `HttpResponse`, `LogLevel`). `PROTOCOL_VERSION = 1`. The
rule in [AGENTS.md](../../AGENTS.md): only add optional fields; anything
else bumps the version and updates `docs/modules/`.

A new *method* that Glossa only sends to modules which declare they handle
it is additive in the same sense as a new optional field: no existing module
ever receives it. Section 3 relies on that.

## 2. Prior art

| system | how it does it | borrow | avoid |
|---|---|---|---|
| **Raycast extensions** | `package.json` declares commands and typed `preferences` (textfield, password, checkbox, dropdown, file); the UI is React, but rendered by Raycast into its own native views from a fixed set of components (`List`, `Detail`, `Form`, `ActionPanel`) | a fixed vocabulary of high-level components rendered natively, never pixels from the extension; typed preferences with `required` that block the command until filled | React and a Node runtime: far too heavy here |
| **Alfred workflows** | `info.plist` plus a visual graph; Script Filters print JSON items Alfred renders; Workflow Configuration (textfield, popup, checkbox, file) is declared, not coded | the script returns *data* (items, a patch), the host decides how to draw it; declared user configuration | the plist and the graph editor |
| **Obsidian plugins** | `manifest.json` (`id`, `version`, `minAppVersion`) and `main.js` with full DOM and Node access; settings tabs built imperatively (`new Setting(el).addText(...)`) | `minAppVersion` | full trust and imperative UI code: every plugin looks different and can do anything |
| **VS Code** | `package.json` `contributes` (commands, `configuration` as JSON Schema, menus, keybindings), `activationEvents` for lazy loading, `when` clauses; webviews for custom UI with message passing and CSP | contribution points; configuration as a schema; lazy activation; a tiny condition language (`when`) | webviews: Glossa has no web engine and should not get one |
| **Home Assistant config flows** | a flow is a list of steps; each step returns a form schema; the code returns `errors` keyed by field; labels come from `strings.json` / `translations/<lang>.json` keyed by step and field id | field-keyed errors returned by code; translations keyed by ids, with the manifest text as the English default | multi-step wizards are more than Glossa needs now |
| **pot-app plugins** | `info.json` with `needs: [{key, display, type: input/select}]`, one `main.js` | already supported; its simplicity is the bar to meet: a small module must stay a small file | |
| **Adaptive Cards** (Microsoft) | a JSON card (`Input.Text`, `Input.ChoiceSet`, `Action.Submit`, containers) that each host renders natively with its own "host config" theme | closest model to what is wanted: a host-rendered element vocabulary, actions that send input values back, a version number on the card and a fallback for unknown elements | |
| **JSON Forms** | a data schema (JSON Schema) plus a separate UI schema (`VerticalLayout`, `Group`, `Control` with `scope: "#/properties/x"`), and `rule`s that show, hide, enable or disable | data separate from layout; layout refers to fields by id; show/enable rules | JSON Pointer scopes and full JSON Schema are heavy to write by hand |
| **react-jsonschema-form** | everything derived from the JSON Schema, `uiSchema` hints only override (`ui:widget`, `ui:help`) | defaults that need no layout at all; hints only where wanted | |
| **Flutter / Compose / SwiftUI** | UI is a function of state; events update state; the framework redraws | one state object per panel; handlers return state patches; the host redraws. egui is immediate-mode, so "redraw from state" is how it already works | |

What this adds up to: a fixed, host-rendered vocabulary (Raycast, Adaptive
Cards); fields declared once and referred to by id from an optional layout
(JSON Forms, rjsf); handlers that receive state and return patches and
field errors (HA, Compose); a tiny condition language (VS Code `when`);
translations keyed by id (HA). Nothing that draws its own pixels or runs a
browser.

## 3. The format

### Why TOML, in `module.toml`

- Modules already have `module.toml`; one file is easier than two, and the
  existing `[[options]]` become the first part of the UI unchanged.
- Glossa's own `config.toml` is TOML, the `toml` crate is already a
  dependency, comments are allowed, and the files are hand-edited.
- TOML is poor at deep nesting. The format answers that by staying flat:
  **elements are a flat list, containers refer to their children by id**
  (as Alfred's graph and JSON Forms' scopes do). This also makes the file
  easy to diff and easy for a future visual builder to rewrite.
- JSON (no comments) and YAML (indentation-sensitive, a heavy parser, and
  a new format in the project) were considered and rejected. KDL reads
  nicely for trees but is unknown to most authors.

A module may move the UI into a separate `ui.toml` with the same tables
when the manifest gets long; Glossa merges it into `module.toml`. This is a
convenience, not a second format.

### The full example: a custom translator

A translator for a fictional LLM API: an API-key field, a model select
whose choices can be loaded from the service, a "Test" button, an
"Advanced" group, and a playground panel with a source box, a language
picker and a result view.

```toml
capabilities = ["translate"]

[module]
id = "acme-translate"
name = "Acme Translate"
version = "1.2.0"
api = 1
runtime = "js"
entry = "main.js"
description = "Translation with Acme's language models."
min_glossa = "0.9.0"                 # new, optional: refuse to load in older builds

[permissions]
network = ["api.acme.example"]

[ui]
format = 1                           # version of everything under [ui] and [[actions]]

# ---- Settings: persisted in config.toml, sent with every request ----------

[[options]]
key = "api_key"
type = "secret"
label = "API key"
help = "Create one at https://acme.example/keys"
required = true
pattern = '^ak-[A-Za-z0-9]{32}$'
pattern_message = "An Acme key is ak- followed by 32 letters or digits."

[[options]]
key = "model"
type = "select"
label = "Model"
default = "acme-small"
choices = [
  { value = "acme-small", label = "Small (fast)" },
  { value = "acme-large", label = "Large" },
]
choices_from = "list_models"         # this action can replace the list
enabled_if = "options.api_key"       # truthy: filled in

[[options]]
key = "temperature"
type = "number"
widget = "slider"
label = "Creativity"
min = 0.0
max = 1.0
step = 0.1
default = 0.2

[[options]]
key = "tone"
type = "select"
label = "Tone"
default = "neutral"
choices = [{ value = "neutral" }, { value = "formal" }, { value = "casual" }]

[[options]]
key = "custom_prompt"
type = "multiline"
label = "Extra instructions"
max_length = 2000
visible_if = "options.tone == 'casual'"

# ---- Actions: named things a button, a field or a panel can trigger -------

[[actions]]
id = "list_models"
label = "Load models"                # handler defaults to the id: list_models() in main.js

[[actions]]
id = "test"
label = "Test connection"
needs = ["api_key"]                  # disabled, with a hint, until these validate

[[actions]]
id = "translate_now"
label = "Translate"
call = "translate"                   # no code: run this module's own translate
args = { text = "state.source", to = "state.to" }
into = "state.result"
shortcut = "Ctrl+Enter"

[[actions]]
id = "explain"
label = "Explain the choice of words"
handler = "explainWords"             # a function in main.js

# ---- Settings page layout (optional; without it, options in order) ------

[ui.settings]
layout = ["api_key", "model", "@list_models", "advanced", "@test"]

[[ui.settings.items]]
id = "advanced"
type = "group"
label = "Advanced"
collapsed = true
children = ["temperature", "tone", "custom_prompt"]

# ---- A panel of its own ----------------------------------------------------

[[ui.panels]]
id = "playground"
title = "Acme playground"
open_from = ["tray", "main_menu"]    # where Glossa offers to open it; also a hotkey action
size = [560, 440]
layout = ["source", "bar", "result", "usage"]

[ui.panels.state]                    # the panel's state, with defaults; lost on close
source = ""
to = "ru"
result = ""
usage = ""

[[ui.panels.items]]
id = "source"
type = "textarea"
bind = "state.source"
placeholder = "Text to translate"
rows = 6
on_submit = "translate_now"          # Ctrl+Enter in the box

[[ui.panels.items]]
id = "bar"
type = "row"
children = ["to", "@translate_now", "@explain"]

[[ui.panels.items]]
id = "to"
type = "lang"
bind = "state.to"
allow_auto = false

[[ui.panels.items]]
id = "result"
type = "result"
kind = "translation"                 # drawn like the popup's result
bind = "state.result"

[[ui.panels.items]]
id = "usage"
type = "label"
bind = "state.usage"
style = "weak"
```

`main.js`, the logic. `translate` is today's entry point, unchanged; the
new functions are action handlers.

```js
function translate(req, host) {
  var r = acme(host, req.options, "/v1/translate", {
    model: req.options.model, text: req.text, target: req.to,
    temperature: req.options.temperature, tone: req.options.tone,
    instructions: req.options.custom_prompt || undefined
  });
  return { text: r.translation, detected_from: r.source_language };
}

// [[actions]] id = "list_models": returns new choices for the select.
function list_models(ev, host) {
  var r = acme(host, ev.options, "/v1/models");
  return { choices: { model: r.models.map(m => ({ value: m.id, label: m.display_name })) } };
}

function test(ev, host) {
  var r = host.fetch("https://api.acme.example/v1/me",
                     { headers: { Authorization: "Bearer " + ev.options.api_key } });
  if (r.status === 401) return { errors: { api_key: host.t("bad_key") } };
  if (!r.ok) throw new ModuleError("http", "status " + r.status);
  return { notice: host.t("connected", r.json().plan) };
}

function explainWords(ev, host) {
  var r = acme(host, ev.options, "/v1/explain",
               { text: ev.state.source, translation: ev.state.result, target: ev.state.to });
  return { state: { usage: r.usage.total_tokens + " tokens" }, notice: r.explanation };
}

function acme(host, options, path, json) {
  if (!options.api_key) throw new ModuleError("missing_credential", "api_key");
  var r = host.fetch("https://api.acme.example" + path, {
    method: json ? "POST" : "GET",
    headers: { Authorization: "Bearer " + options.api_key },
    json: json
  });
  if (r.status === 429) throw new ModuleError("rate_limited", "slow down");
  if (!r.ok) throw new ModuleError("http", "status " + r.status);
  return r.json();
}
```

`locales/ru.toml`, the Russian strings (see [i18n](#i18n)):

```toml
[options.api_key]
label = "API-ключ"
help = "Создайте ключ на https://acme.example/keys"
pattern_message = "Ключ Acme: ak- и ещё 32 буквы или цифры."

[options.model]
label = "Модель"
choices = { acme-small = "Маленькая (быстрая)", acme-large = "Большая" }

[actions.list_models]
label = "Загрузить модели"

[actions.test]
label = "Проверить подключение"

[panels.playground]
title = "Песочница Acme"

[panels.playground.items.source]
placeholder = "Текст для перевода"

[strings]                            # host.t("bad_key") in code
bad_key = "Ключ не подошёл."
connected = "Подключено, тариф: {0}."
```

A module with no `[ui]` and no `[[actions]]` is exactly today's module.
A module with only richer `[[options]]` gets a better settings page and no
code changes.

### The element schema

Every element has `id` (unique within its page or panel) and `type`.
Options are elements too: an option key may appear in any layout, and is
drawn with its own type. In a `layout` or `children` list a bare id names an
element or an option, and `@id` names an action, drawn as its button.

Common keys:

| key | meaning |
|---|---|
| `label`, `help`, `placeholder` | text; `help` is the hover tooltip, as on today's settings rows |
| `bind` | `state.<key>` or `options.<key>`: where the value lives (options bind to themselves) |
| `visible_if`, `enabled_if` | a [condition](#conditions) |
| `on_change`, `on_submit`, `on_click` | an action id ([events](#events)) |
| `width` | `"fill"` or points; default is the row's natural width |

The vocabulary, version 1:

| `type` | value | notes |
|---|---|---|
| `label` | string (or bound) | `style = "normal" \| "weak" \| "error" \| "heading"` |
| `link` | URL | opens in the browser; only `http`/`https`; the URL shows on hover |
| `separator` | | |
| `text` | string | single line; `password = true` masks |
| `textarea` | string | `rows`, `monospace` |
| `secret` | string | only for options: stored like `type = "secret"` options today |
| `number` | number | `min`, `max`, `step`; `widget = "slider"` for a slider |
| `toggle` | bool | |
| `select` | string | `choices`, `choices_from` |
| `multiselect` | list of strings | checkboxes in a popup |
| `lang` | language code | Glossa's language picker; `allow_auto` |
| `file` | path | a file dialog (`rfd`, already a dependency); `filters`; the module gets the path, not the file |
| `button` | | an action's button; `@id` in a layout is the short form |
| `row` | container | children side by side |
| `group` | container | a titled frame like the Options pages' group boxes; `collapsed` |
| `tabs` | container | `children` are `group`s, each a tab |
| `result` | bound | `kind = "translation" \| "article" \| "text" \| "markdown" \| "table" \| "json"` |
| `list` | bound list | rows of `{ title, subtitle, actions }`, like an Alfred or Raycast list; clicking a row runs an action with the row as `ev.item` |
| `image` | bound | base64 PNG/JPEG or an image handle; for OCR modules |
| `progress` | bound number or `null` | `null` spins |

Unknown types (from a newer `format`) are drawn as a weak "needs a newer
Glossa" line instead of failing the module, as Adaptive Cards falls back.

### Option keys added

On top of today's `OptionSpec` keys, all optional:

| key | for |
|---|---|
| `pattern`, `pattern_message` | a regular expression the value must match (the `regex` crate, already in the workspace) |
| `min_length`, `max_length` | text length |
| `step`, `widget` | numbers: drag step, `"slider"` |
| `choices_from` | an action whose reply may set `choices.<key>` |
| `visible_if`, `enabled_if` | conditions |
| `group` | shorthand for putting an option into a `group` element without a layout |
| new `type`s | `lang`, `file`, `multiselect` |

The `OptionSpec` struct grows these as `#[serde(default)]` fields; built-in
providers can use them too.

### Data binding

Two stores, both plain JSON objects:

- **`options`**: the module's settings, as today. Persisted in
  `config.toml`. On the settings page, edits go into Options' draft and are
  saved with *Apply*, like every other setting.
- **`state`**: per open panel, initialised from `[ui.panels.state]`,
  dropped when the panel closes. `persist = ["to"]` on a panel keeps the
  listed keys in the module's cache between openings.

Widgets read and write their bound key directly; no round trip to the
module. The module sees both stores in every action call.

### Events

Events name an action. An action is either:

- **a handler** (`handler`, defaulting to the action's id): module code;
- **a capability call** (`call = "translate" | "dictionary" | "speak" | "detect"`):
  the engine runs the module's own method through the normal pipeline, with
  `args` mapping request fields to bound values, and writes the result to
  `into`. The playground panel above needs no code at all for its
  *Translate* button;
- **a built-in**: `builtin = "copy"` (copies a bound value),
  `builtin = "open_url"`, `builtin = "speak"` (Glossa's current voice).

Which events exist:

| event | on | when |
|---|---|---|
| `on_click` | buttons, list rows | clicked |
| `on_submit` | `text`, `textarea` | Enter (Ctrl+Enter in a textarea) |
| `on_change` | any input | the value changed and stayed unchanged for 400 ms; off unless set, since each one is a module call |
| `on_open` | a panel | the panel opened |
| `shortcut` | an action | while its panel has focus |

Actions run on a worker thread, never on the UI thread. While one runs, its
buttons show a spinner and are disabled; a second click is ignored, closing
the panel cancels the call.

### The `action` method

One new protocol method. Glossa sends it only to modules that declare
handler actions, so no existing module ever sees it and `api` stays 1.

Params (`wire::ActionParams`):

```json
{ "action": "explain",
  "handler": "explainWords",
  "panel": "playground",
  "event": { "type": "click", "element": "explain" },
  "state": { "source": "Hello", "to": "ru", "result": "Привет", "usage": "" },
  "options": { "api_key": "ak-…", "model": "acme-small", "temperature": 0.2 },
  "ui_lang": "ru" }
```

`panel` is absent for actions on the settings page (whose `options` are the
draft, so "Test" checks the values the user typed and has not yet applied);
`event.item` carries the clicked row of a `list`.

Reply (`wire::ActionReply`), every field optional:

```json
{ "state":   { "usage": "312 tokens" },
  "options": { "model": "acme-large" },
  "choices": { "model": [{ "value": "acme-xl", "label": "XL" }] },
  "errors":  { "api_key": "The key was refused." },
  "notice":  "Connected.",
  "notice_level": "info" }
```

- `state` and `options` are shallow merges. `options` is honoured only on
  the settings page and lands in the draft: a panel cannot change settings
  behind the user's back.
- `errors` are shown under the named fields, in the error colour, until
  the field changes (HA config flows do the same).
- A thrown `ModuleError` shows as a notice with the usual
  `describe` wording; `missing_credential` with an option key focuses that
  field.

Per runtime:

| runtime | what the author writes |
|---|---|
| JavaScript | a global function named by `handler`, `(ev, host)`, returning the reply (or a promise of it) |
| Python | `@action("explain")` on a function in `glossa_module.py`'s `Module` |
| Java | `@Action("explain")` on a method, in `sdk/java` |
| process | handle `"method": "action"` in the JSON-lines loop |
| native | `call("action", params_json)`; `glossa-module-sdk` gets an `action` method on the `Module` trait with a default that returns `unsupported` |

**Streaming.** A long handler (an LLM explaining a sentence) may send
partial state while it runs through a new host call, `ui_update`
(`host.ui.update({ result: "…so far" })` in JavaScript), which the panel
applies at once. Host calls are already forwarded untouched by the stdio
transport, so it reaches every runtime.

**Strings.** `host.t(key, ...args)` returns the module's string from
`[strings]` in the user's UI language. JavaScript and the SDKs resolve it
locally from a table passed with the call (`ui_lang` plus the module's
loaded strings), so it costs no round trip; a `t` host call exists for
runtimes without an SDK.

**Engine first.** Following the "everything below the UI" rule, panel state,
action dispatch, validation and condition evaluation live in
`glossa-engine` (a new `panels` module), not in the UI crate. The UI only
draws elements and forwards events. That gives the CLI and the control API
the same reach:

```bash
glossa-cli module action acme-translate test --option api_key=ak-…
glossa-cli module action acme-translate explain --panel playground --state source=Hello --state to=ru
```

and the module test harness can drive actions as well as methods.

### Conditions

`visible_if` and `enabled_if` take a tiny expression, evaluated natively
every frame (no module call):

```
options.api_key                     truthy
!state.result                       falsy
options.tone == 'casual'            equality with a string, number or bool
options.temperature > 0.5           < <= > >= on numbers
options.mode == 'custom' && options.url
```

Grammar: a path (`options.*` / `state.*`), a literal, `== != < <= > >=`,
`!`, `&&`, `||`, parentheses. No function calls, no arithmetic. A parser of
this size is around 150 lines and is tested against a table of cases. JSON
Forms' rule objects were considered; an expression is shorter in TOML and
reads like the `when` clauses VS Code authors know.

### Validation

Declarative rules run natively, every frame, with no module call:
`required`, `pattern`, `min`/`max`, `min_length`/`max_length`, and the type
itself (a `url` must parse). A field that fails shows its message (the
`pattern_message`, or a built-in one through the locale) under it.

An action with `needs = [...]` is disabled until those fields validate, with
a tooltip naming what is missing. *Apply* on the settings page still saves
invalid values (the user may be half-way through), but the module's entry in
the services list gets the same "needs setup" mark a missing `required`
option gets today.

Checks only the service can do (is the key accepted?) are an action that
returns `errors`, as `test` does above. `[ui.settings] validate = "test"`
runs it on *Apply* as well.

### i18n

Manifest text is the English default. Translations live in
`locales/<lang>.toml` in the module folder, keyed by where the string
appears:

| key | string |
|---|---|
| `options.<key>.label` / `.help` / `.placeholder` / `.pattern_message` | an option |
| `options.<key>.choices.<value>` | a choice's label |
| `actions.<id>.label` | an action's button |
| `settings.items.<id>.label` | a settings-page element |
| `panels.<id>.title`, `panels.<id>.items.<id>.label` / `.placeholder` | a panel |
| `strings.<key>` | strings for code, through `host.t` |
| `module.name`, `module.description` | the listing |

The language is Glossa's UI language (`en`, `ru`, or the code of a classic
`lang.json` pack). A missing key falls back to the manifest. Placeholders are
`{0}`, `{1}`, like Glossa's own `locale.fmt_args`. The editor can show which
keys a locale file lacks.

### Versioning

| what | how |
|---|---|
| the module | `[module] version`, as today |
| the protocol | `api = 1` stays: everything here is optional fields, one new method sent only to modules that declare actions, and one new host call. The registry's exact-match check on `api` is not touched |
| the UI format | `[ui] format = 1`. A Glossa that knows a lower format draws what it knows and shows unknown element types as placeholders; unknown keys are ignored |
| Glossa itself | `min_glossa = "x.y.z"`: an older build lists the module under "failed" with "needs Glossa x.y.z or newer" |
| older Glossa builds | ignore `[ui]` and `[[actions]]` entirely (no `deny_unknown_fields`) and load the module as a plain service |

Should a future change need something incompatible, `format = 2` is the
switch, and Glossa keeps rendering format 1 alongside it.

### How it maps to egui

Everything is drawn with the widgets Options already uses and the colours
of `style::Look`, so a module's page cannot be told apart from Glossa's own.

| element | egui | `Look` / Glossa helper |
|---|---|---|
| option row | `widgets::row(ui, label, …)`: label left, control right | `look.text()`; required mark ` *` as today |
| `help` | `.on_hover_text()` | |
| `label` | `egui::Label` / `bidi::label` for right-to-left text | `look.text()`, `look.weak_text()` (`weak`), `look.error()` (`error`) |
| `text`, `secret`, `textarea` | `egui::TextEdit` (`.password()` for secrets) | `look.edit_back()`; the page's "show secrets" switch applies |
| `number` | `egui::DragValue` with `.range()`; `egui::Slider` for `widget = "slider"` | `look.accent()` for the slider's fill |
| `toggle` | `ui.checkbox` | |
| `select`, `multiselect` | `crate::dropdown::ComboBox`, which respects `SHELL`'s rules for dropdowns inside a window | `look.overlay_back` for the list |
| `lang` | `widgets::lang_combo` (searchable, the same picker as the main window) | |
| `file` | a `TextEdit` and a "Browse…" button opening `rfd` on a worker | |
| `button` | `egui::Button`; the first action in a row is the default, filled with `look.accent()` | spinner (`egui::Spinner`) while running |
| `group` | `widgets::group(ui, look, title, …)`; `collapsed` uses `egui::CollapsingHeader` | `look.theme.window.group_box` border |
| `row` | `ui.horizontal` | |
| `tabs` | the service-tab style of `widgets::service_tab` | |
| `separator` | `ui.separator()` | `look.border()` |
| `result: translation` | the popup's result view (`popup.rs`), with copy and speak buttons | the popup's fonts and `text_scale` |
| `result: article` | `rich.rs`, the dictionary window's article renderer (structured and simplified-HTML articles) | |
| `result: markdown` | a small Markdown subset (paragraphs, bold, italics, lists, code, links) to `LayoutJob` | |
| `result: table`, `list` | `egui::Grid` with striped rows | `look.weak_text()` for subtitles |
| `image` | `egui::Image` from a texture loaded on demand | |
| field error | a small label under the field | `look.error()` |
| panel window | a deferred viewport like the dictionary and history windows, with Glossa's own title bar where `SHELL` allows it, titled "Module name: panel title" | the window backdrop and transparency settings |

No module text is ever drawn with an emoji or symbol glyph that the fonts
may lack: icons in module UIs are not part of format 1.

Where panels open from:

- `open_from = ["tray"]`: an entry under a "Modules" submenu of the tray
  menu (the submenu appears only when some module has a panel);
- `"main_menu"`: the main window's menu;
- always: a hotkey action `open_panel` with the panel's `module/panel` id,
  the control API (`POST /panel`) and `glossa-cli module panel`.

Memory: a panel's description is parsed with the manifest (a few KB); its
state exists only while it is open; nothing extra stays resident in the
tray. When the GUI unloads, open panels close with the other windows, and
their `persist` keys are written to the cache.

## 4. The scripting language

The question is which language, if any, Glossa should embed *in addition*
for module logic, given that feature A works with any runtime through the
`action` method.

### Measurements

Measured on 2026-11-07 on an Apple Silicon Mac, in a throwaway project with
Glossa's release profile (`opt-level = "z"`, fat LTO, one codegen unit,
`panic = "unwind"`, stripped). Each binary creates one engine, runs a loop
of a million additions and sleeps. "Added size" is over the same program
with no engine (0.32 MB, which includes `serde_json`); "peak memory" is the
maximum resident set over the base program's 1.8 MB. Loop times are the
range over four runs on a machine busy with other builds, so read them as
ratios. Windows x86-64 sizes differ somewhat; the ordering does not.

| engine | crate | added size | peak memory added | 1M-iteration loop |
|---|---|---|---|---|
| boa (today) | `boa_engine` 0.22 | +3.83 MB | +2.8 MB | 157–723 ms |
| Lua 5.4 | `mlua` 0.11, `lua54` + `vendored` | +0.23 MB | +0.3 MB | 5–10 ms |
| Luau | `mlua` 0.11, `luau` | +0.93 MB | +1.3 MB | 8–16 ms |
| Rhai | `rhai` 1 | +1.22 MB | +1.2 MB | 95–440 ms |
| QuickJS (for comparison) | `rquickjs` 0.10 | +0.52 MB | +0.8 MB | 91–245 ms |
| wasmi (interpreter only) | `wasmi` 2 | +0.60 MB | +0.5 MB | (not comparable: runs compiled code) |
| wasmtime | | estimated 10 MB or more with Cranelift; not measured | | |

Creating a context, running a one-line script and dropping it, which is
what Glossa does on every JavaScript call: boa 1.1–1.5 ms, QuickJS
0.5–0.8 ms, Lua 5.4 about 0.1 ms (200 iterations, three runs each).

Boa's size is already inside `glossa.exe` (shared dependencies make its real
share there somewhat smaller than measured here). Every other row would be
added to a program of about 20 MB.

Idle memory is a separate question from peak memory. Glossa creates a JS
context per call and drops it, so installed JavaScript modules cost nothing
between calls. Any embedded engine can be used the same way (a Lua state is
a few tens of KB and is created in about a tenth of a millisecond), so **no candidate
costs idle memory if used per call**; the differences are binary size and
the cost while a call runs.

### The candidates

**JavaScript on boa (already embedded).**

- Size: nothing added.
- Speed: the slowest engine measured: about three times slower than
  QuickJS on a tight loop and tens of times slower than Lua. For module work (build a request, read JSON, reshape it)
  the network dominates: the bundled modules do all their work this way.
- Language: modern ES (classes, `async`, destructuring, Unicode regex);
  no `Intl`; one file; no npm runtime.
- Sandbox: no I/O except the host; loop and recursion limits; the call
  timeout. Missing: a heap limit.
- Authoring: the language most module authors and AI assistants know;
  the pot-app and QTranslate ecosystems are JavaScript; documentation and a
  `glossa.d.ts` for editor completion are cheap to add.

**Lua 5.4 via mlua.**

- Size: the smallest added cost measured. Needs a C compiler for every
  target (the Windows build cross-compiles from WSL with MinGW, which
  works, but it is a new C dependency in the build).
- Speed: much faster than boa.
- Sandbox: good, if set up deliberately: load only `string`, `table`,
  `math`, `utf8` (`Lua::new_with`), never `io`, `os`, `debug`, `package`;
  `set_memory_limit` caps the heap; an instruction-count hook stops runaway
  loops.
- Authoring: simple, but unfamiliar to most of Glossa's likely authors;
  1-based arrays; no JSON in the language (the host would provide it).
  A second language means a second set of docs, examples and SDK
  helpers, and a second highlighter in the editor.

**Luau via mlua.** Designed for sandboxing (Roblox), with gradual types and
an interrupt callback. About four times the added size of Lua 5.4, C++
rather than C. The types are nice; the size and the C++ toolchain are not.

**Rhai.** Pure Rust, no C toolchain, excellent built-in limits
(`set_max_operations`, string, array and map size limits). But it is the
largest of the small engines here, slower than Lua, and a language almost
no module author knows already. Rejected.

**WASM.** `wasmtime` is far too large for Glossa. `wasmi` is small and gives
the strongest isolation (fuel metering, memory limits, no ambient
authority). But authors need a compiler toolchain (Rust, TinyGo,
AssemblyScript), there is no in-app editing story (the editor of feature B
cannot compile Rust), and the component model, which would make a typed
interface pleasant, is not what wasmi offers. A possible later runtime for
heavy, untrusted, compiled code; not the answer to "easy to write".

**Any language over stdio (today's process runtime).** Already there, and
the `action` method makes it work for UIs as well: a Python or Go module
gets the same native panel. Costs nothing in the binary. But it needs an
installed runtime, runs with the user's full rights, and the in-app editor
can only edit and restart it, not provide a no-install experience.

### Recommendation

**Use JavaScript on boa as the one embedded language, and add none.**

- It costs no bytes. Every alternative adds a quarter of a megabyte to a
  megabyte and a half to a binary whose size is a stated feature.
- Per-call contexts already give zero idle memory.
- The sandbox exists, is tested and is documented.
- One language means one SDK surface, one set of docs and examples, one
  highlighter in the editor, and compatibility with pot-app and QTranslate
  scripts users already have.
- Speed does not matter for the work modules do; when it does, a process
  or native module is the right tool.
- Feature A is runtime-neutral anyway: Python, Java, process and native
  modules implement `action` and get the same UI.

Two follow-ups this choice makes worth doing:

1. **A heap limit for JS calls.** Boa has none. Until it does, the call
   timeout bounds a runaway allocation in time but not in size.
2. **QuickJS as a future swap, not an addition.** `rquickjs` measured
   smaller (+0.52 MB against boa's +3.83 MB) and two to three times faster than
   boa, and QuickJS has a memory limit and an interrupt handler. Replacing
   boa would save around 3 MB and close the heap-limit gap, at the price of
   a C dependency and re-testing the three JS dialects. That is its own
   decision, outside this RFC; the module format does not depend on it.

   *Update, November 2026:* done. JavaScript now runs on QuickJS through
   `rquickjs`, with a 128 MB heap limit and a 10-second script-time limit
   per call; `glossa` shrank by 3.1 MB on macOS (see the CHANGELOG). The
   rest of this RFC still says boa where it describes the state it was
   written in.

Lua is the fallback if a measured need appears (for example, heavy text
processing in modules that must not need an installed runtime). It is not
part of the plan.

## 5. The editor

### Highlighting options

Measured in the same throwaway project (added size over the base program):

| option | added size | notes |
|---|---|---|
| `egui::TextEdit` + own highlighter | ~0 (a few hundred lines) | a `layouter` that turns tokens into a `LayoutJob`; Glossa already uses a custom layouter for the bidi-aware source editor (`bidi::editor_layout`) |
| `egui_extras::syntax_highlighting` | not measured; small without syntect | a minimal built-in highlighter for a few languages; not JavaScript-aware enough to be worth a dependency Glossa does not otherwise use |
| `egui_code_editor` | not measured; small | `TextEdit` with line numbers and a simple built-in highlighter for several languages; has to publish a release for every egui version, so an egui upgrade in Glossa would wait on it |
| syntect (`default-fancy`, built-in syntaxes) | +1.43 MB, and +12 MB resident once the syntax set is loaded | Sublime grammars and themes; accurate; slow to load the syntax set; far too large for two languages |
| tree-sitter + highlight + JS and Lua grammars | +1.39 MB, +3.2 MB peak memory | accurate, incremental; each grammar is compiled C; a C dependency in the build |

**Recommendation: own highlighter on `TextEdit`.** Two languages need
highlighting (JavaScript and TOML), plus JSON for test fixtures. A
hand-written tokenizer for each (keywords, strings including template
literals, numbers, comments, regex literals for JS; tables, keys, strings,
values for TOML) is a few hundred lines with tests, costs nothing in size,
and uses `Look` colours, so it follows the theme. Tokenizing a 2,000-line
file is well under a millisecond, and the `LayoutJob` is cached by a hash of
the text, so idle frames cost nothing. If a third language is ever needed
(Python for process modules), it is another small tokenizer.

What the editor widget (`glossa-ui-egui/src/code_editor.rs`) adds over
`TextEdit`:

- a line-number gutter drawn beside it, scrolled with it;
- monospace font (egui's built-in), tab inserts spaces, auto-indent on
  Enter, Ctrl+/ toggles comments;
- error underlines: the layouter gives a span `look.error()` underline, and
  the gutter shows a mark on the line;
- find (Ctrl+F) over the buffer; go to line (Ctrl+G).

What it does not do in the first version: completion, multiple cursors,
folding. `TextEdit` keeps undo and redo.

**Lazy loading.** The editor code is compiled in but costs nothing until
opened: no buffers, no caches, no window. Like every other window it goes
away when the GUI unloads (unsaved buffers are kept in memory by the
engine-side editor state, or the window asks before closing).

### The window

A window of its own, *Module editor*, opened from *Options → Modules*:
"New module…" (from a template: translator, dictionary, OCR) or "Edit" on a
module in the user's modules folder. Bundled and system-wide modules offer
"Copy to my modules and edit", which uses the replace-by-id mechanism that
already exists for bundled modules.

```
┌ Module editor: Acme Translate ────────────────────────────────────────┐
│ [Save] [Test ▸] [Open folder]                       acme-translate 1.2 │
├────────────┬───────────────────────────────┬──────────────────────────┤
│ module.toml│  1 capabilities = ["translate"]│  Preview: [Settings ▾]   │
│ main.js    │  2                             │  API key *  [••••••••]   │
│ locales/   │  3 [module]                    │  Model      [Small  ▾]   │
│   ru.toml  │  4 id = "acme-translate"       │  [Load models]           │
│ tests/     │  5 name = "Acme Translate"     │  ┌ Advanced ──────────┐  │
│   basic.toml│ …                             │  │ Creativity  ──o─── │  │
│            │ 22 pattern = '^ak-[A-Z…'       │  └────────────────────┘  │
├────────────┴───────────────────────────────┴──────────────────────────┤
│ Problems (1) │ Log │ Test results                                     │
│ ● main.js:14:9  ReferenceError: optons is not defined                 │
└───────────────────────────────────────────────────────────────────────┘
```

- **Files** on the left: the module folder's text files. Binary files
  (a `.jar`, a library) are listed but not opened.
- **Editor** in the middle, one tab per open file.
- **Preview** on the right: the settings page or any panel, drawn by the
  same renderer as the real thing, with the current state. Buttons in the
  preview run actions for real (the user is testing their own module), so
  the preview is how "Load models" gets tried.
- **Bottom drawer**: *Problems* (every error with `file:line:col`; a click
  jumps there), *Log* (the module's `host.log` and `console.*` lines from
  the preview and tests, which today only reach `glossa.log`), *Test
  results*.

On a narrow window the preview folds under the editor.

### Edit, reload, test

1. **Save** (Ctrl+S) writes the file (to a temporary name, then renamed).
2. **Check**: the engine re-reads the manifest, and reports a TOML error
   with its span (the `toml` crate gives byte ranges, turned into line and
   column), then a semantic error (an unknown option referenced in a
   layout, a condition that does not parse, a handler action whose function
   is missing from `main.js`), then the script's syntax through the
   existing `check_syntax`.
3. **Reload** only this module: a registry call that replaces one provider
   by id, instead of today's *Reload modules*, which rescans everything.
   The preview redraws with the new manifest; panel state is kept where the
   keys still exist.
4. **Test** (F5) runs the module's tests through the test harness the
   module-devkit work is adding. This RFC assumes that harness is a
   library function below the UI (in `glossa-modules` or `glossa-engine`)
   that takes a module folder and returns per-case results, so the editor
   and `glossa-cli` share it. Each case shows pass or fail, the request,
   the reply and the log; a failed case links to the line that threw where
   the engine can tell.

Process modules (Python, Java, any program) reload by stopping their
process; the next call starts it with the new code. Native modules cannot
be unloaded safely; the editor says "restart Glossa to load the new
library".

**Line numbers for runtime errors.** Syntax errors from boa carry a
position. Errors thrown while a script runs currently reach Glossa as a
message only (`engine::describe`). Feature B needs boa's position or stack
for the thrown error carried into `ModuleError` (a new optional `location`
field: `{ file, line, column }`), and the Python and Java SDKs can fill the
same field from their tracebacks. That is an additive wire change.

## 6. Security

Today's model, which this RFC keeps:

- JavaScript modules are sandboxed: no files, no processes, network only
  through `host.fetch` to the hosts in `[permissions] network`.
- Python, Java and process modules are ordinary programs with the user's
  rights; their own connections are not covered by permissions.
- Native modules run inside Glossa with its full rights and load only when
  `[modules] native` is switched on.

What the declarative format adds, and how it is contained:

**The UI is drawn by Glossa, never by the module.** A module chooses
elements from a fixed list; it cannot draw pixels, place a window over
another program, or imitate Glossa's own dialogs. Every panel's title
starts with the module's name. Links open only `http` and `https` URLs, and
show the URL on hover. A module cannot read other modules' options or
Glossa's settings; a panel cannot change even its own settings (only the
settings page can, through the draft the user applies).

**Permission prompts.** When a module is installed from a folder, a zip or
a `.potext`, and when an update widens what it asks for, Glossa shows what
it will be able to do before it loads:

- "Can reach: api.acme.example" (or "any website" for `*`);
- "Runs a program on your computer with your rights" for Python, Java,
  process and native modules, with the command line;
- "Downloads: model.onnx, 84 MB, from huggingface.co" for declared assets;
- "Adds an entry to the tray menu" for `open_from = ["tray"]`.

The approved set is recorded in `config.toml`
(`[modules.granted.<id>]`, the hosts and the runtime). A module whose
manifest later asks for more stays switched off, with "needs your approval"
in *Options → Modules*, until the user approves the new set: the model of
browser extensions. Modules created in the editor are approved by their
author by definition; bundled modules are trusted.

**Hash-pinned assets.** The module-assets work adds downloads checked
against a SHA-256 before they are used. The declarative format only needs
two things from it: the asset list is declared in `module.toml` (so the
prompt can list it, with host and size, before anything is fetched), and a
module refers to an asset by name, never by a URL its code chose at run
time. A download goes through Glossa's HTTP client and proxy, to a host
that must be in the module's network permissions.

**Secrets.** Options of type `secret` keep their current treatment
(keychain, masked fields). In addition: secret values are never written to
the log by the action machinery, are masked in the editor's preview and
log panes, and test fixtures in a module folder must not contain real keys
(the harness reads keys from the environment or the user's settings).

**Resource limits.** Actions run on workers, one at a time per panel, with
the module call timeout; `on_change` is debounced and opt-in; JavaScript
keeps its loop and recursion limits. The missing JS heap limit is the open
gap ([section 4](#recommendation)).

**Out of scope:** signing modules and a module catalogue. Both make sense
only once there is somewhere to download modules from; the grant records
above are the hook a catalogue would use.

## 7. Plan

Sizes are rough, counting code, tests and docs.

| phase | delivers | size | depends on | parallel with |
|---|---|---|---|---|
| **0. Decide** | this RFC reviewed; format 1 frozen to what phase 1 and 2 need | — | — | — |
| **1. Richer settings** | new `OptionSpec` keys (`pattern`, lengths, `step`, `widget`, `visible_if`/`enabled_if`, `group`, `lang`/`file`/`multiselect` types); the condition parser; native validation with messages under fields; `[ui.settings]` layout with `group`/`row`/`tabs`; `locales/<lang>.toml`; `min_glossa`; docs in `docs/modules/README.md`. No protocol change. Every runtime, and built-in providers, benefit at once | ~1,200 lines | 0 | 2, 4a |
| **2. Actions** | `[[actions]]`; `wire::ActionParams`/`ActionReply`; the `action` method in the JS dialect, the Python and Java SDKs, the native SDK; `choices_from`; buttons on the settings page; `host.t`; `Engine::module_action`; `glossa-cli module action`; `protocol.md` | ~1,000 lines | 0 (touches `manifest.rs` with phase 1: agree the struct first) | 1, 4a |
| **3. Panels** | `glossa-engine::panels` (state, dispatch, persistence); the panel window; all format-1 elements including `result` kinds reusing the popup and dictionary renderers, `list`, `image`, `progress`; `call` and built-in actions; `ui_update` streaming; tray and menu entries; `open_panel` hotkey action; control API and CLI | ~2,000 lines | 1, 2 | 4a, 5 |
| **4a. Editor widget** | `code_editor.rs`: tokenizers for JS, TOML, JSON; gutter; error underlines; find; tests | ~800 lines | — | 1, 2, 3 |
| **4b. Editor window** | the window, file list, preview (settings page after phase 1, panels after phase 3), save → check → reload-one, Problems/Log/Test drawers, templates, `ModuleError.location` | ~1,500 lines | 4a, 1 (3 for panel preview), the module-devkit harness for tests | 5 |
| **5. Trust** | install and update prompts, `[modules.granted]`, asset declarations in the prompt (with the module-assets work), "needs approval" state | ~700 lines | 2 (for the full manifest) | 3, 4b |
| later, optional | a JS heap limit or the QuickJS swap; a visual layout builder in the editor; a module catalogue with signing | — | — | — |

What each step is worth on its own:

- After **1**, every existing module can have a properly laid out,
  validated, translated settings page with no code.
- After **2**, modules get "Load models" and "Test" buttons like the
  built-in AI endpoints, in any runtime.
- After **3**, modules get windows of their own.
- After **4b**, a module can be written from a template to a working
  service without leaving Glossa.
- **5** should land before modules are shared widely, but nothing in 1–4
  makes today's trust model worse.

Size budget: the whole plan should add well under 0.5 MB to `glossa.exe`
(no new dependencies are proposed), and nothing to resident memory in the
tray.

## 8. Open questions

- **Where do panels live besides their own window?** A tab in the main
  window, or a section of the popup, would make some modules (a glossary
  lookup, a grammar checker) feel built in. Format 1 leaves room
  (`open_from` takes new values) without committing.
- **Should `call` actions write to history?** Translating in a module's
  playground is a translation; probably yes, marked with the panel.
- **A separate `ui.toml`:** worth supporting from the start, or only when
  someone's manifest gets long?
- **The JS heap limit:** patch boa, wait for upstream, or take the QuickJS
  swap?
- **Classic `lang.json` packs:** module strings fall back to English in any
  language other than English and Russian unless the module ships that
  locale. Acceptable?
