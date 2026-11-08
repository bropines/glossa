# Module panels

A module can have windows of its own: a glossary to search, a playground
for an LLM, a word of the day. A panel is described in `module.toml` as a
list of elements (fields, buttons, lists, results); Glossa draws it with
its own widgets and colours, keeps its state, and runs its buttons through
the module's [actions](README.md#actions). There is no drawing code in the
module, and the module cannot draw anything Glossa does not offer.

[`modules/js/pocket-glossary`](../../modules/js/pocket-glossary) is a
complete example: a search field, a list of matching words, the chosen
word's article, a "word of the day" that streams its progress, and copy and
listen buttons, with fixtures and a Russian translation.

## Contents

- [A first panel](#a-first-panel)
- [Where a panel opens](#where-a-panel-opens)
- [State](#state)
- [Elements](#elements)
- [Actions in a panel](#actions-in-a-panel)
- [Streaming](#streaming)
- [Translations](#translations)
- [Limits](#limits)
- [From the command line and the control API](#from-the-command-line-and-the-control-api)
- [Testing](#testing)

## A first panel

```toml
[[actions]]
id = "search"
label = "Search"                     # search(ev, host) in main.js

[[actions]]
id = "look_up"
call = "dictionary"                  # no code: this module's own dictionary
args = { text = "item.title", from = "en" }
into = "state.article"

[[ui.panels]]
id = "glossary"
title = "Glossary"
open_from = ["tray", "main_menu"]
size = [520, 560]
layout = ["query", "@search", "words", "article"]
persist = ["query"]                  # kept between openings
on_open = "search"                   # run when the panel opens

[ui.panels.state]                    # every key the panel holds, with its first value
query = ""
words = []
article = {}

[[ui.panels.items]]
id = "query"
type = "text"
bind = "state.query"
placeholder = "Search the glossary"
on_change = "search"                 # 400 ms after typing stops

[[ui.panels.items]]
id = "words"
type = "list"
bind = "state.words"
on_click = "look_up"                 # the clicked row is ev.event.item

[[ui.panels.items]]
id = "article"
type = "result"
kind = "article"
bind = "state.article"
visible_if = "state.article.headword"
```

```js
function search(ev, host) {
  var q = ev.state.query.toLowerCase();
  var hits = TERMS.filter(function (t) { return t.word.indexOf(q) >= 0; });
  return { state: { words: hits.map(function (t) { return { title: t.word, subtitle: t.meaning }; }) } };
}
```

Elements are a flat list; containers (`row`, `group`, `tabs`) name their
`children` by id, and a layout's `@id` places an action's button. This is
the same scheme as [`[ui.settings]`](README.md#settings-pages).

## Where a panel opens

Every panel's window is titled "Module name: panel title", so it cannot
pass for one of Glossa's own.

| from | how |
|---|---|
| the tray | `open_from = ["tray"]`: an entry in the tray menu's *Modules* submenu (shown only when some module has one) |
| the main window's menu | `open_from = ["main_menu"]` |
| a hotkey | `[hotkeys.panels]` in `config.toml`: `"pocket-glossary/glossary" = "Ctrl+Alt+G"` ([configuration](../configuration.md#hotkeys)) |
| the control API | `POST /v1/panel?id=MODULE/PANEL`, or the action `open_panel` with the name as its text ([control API](../control-api.md#panels)) |
| the command line | `glossa-cli module panel MODULE/PANEL --open` ([cli.md](../cli.md#module)) |
| the module editor | its preview draws the module's panels, live |

A module installed by hand shows its panels only once it is
[approved](README.md#permissions-and-trust): the approval lists each panel
and the tray or menu entry it adds, and an update that adds a panel or an
entry asks again. While a module waits, its panels, entries and hotkeys do
not appear.

A panel costs nothing until it is opened. An open panel is a window like
the dictionary or the history: while it is on screen Glossa keeps its
graphics loaded, and they unload (`unload_graphics_after_secs`) only once
every window, panels included, is closed. A panel closed by the user, or
when Glossa quits, stops its running action and keeps its `persist` keys.

## State

`[ui.panels.state]` declares the panel's state and its values when the
panel opens. Elements read and write their bound key (`bind =
"state.query"`) directly, with no call to the module. Every action call
carries the whole state (`ev.state`), and a reply's `state` is merged into
it, shallowly.

- Only declared keys (and keys an element binds) are kept; anything else a
  module sends is dropped.
- `persist = ["key", …]` keeps those keys in Glossa's cache folder between
  openings (`cache/panels/<module>.<panel>.json`). Everything else is lost
  when the panel closes.
- `bind = "options.<key>"` shows one of the module's settings, read only: a
  panel cannot change settings. A reply's `options` and `choices` are
  ignored in a panel.
- `visible_if` and `enabled_if` read `state.*` and `options.*`, with
  `.field` steps into objects (`state.article.headword`); the grammar is the
  settings pages' ([conditions](README.md#settings-pages)).

## Elements

Common keys: `id`, `type`, `label`, `help` (the tooltip), `placeholder`,
`bind`, `visible_if`, `enabled_if`, `width` (`"fill"` or points) and the
events `on_change`, `on_submit`, `on_click`, each naming an action.

| `type` | shows | keys |
|---|---|---|
| `label` | its `label`, or the bound value as text | `style = "normal" \| "weak" \| "error" \| "heading"` |
| `link` | a link to `url` (or the bound value), `label` as its text | only `http` and `https` addresses open; the address shows on hover |
| `separator` | a line | |
| `text` | a one-line field | `password`, `monospace`; `on_submit` on Enter |
| `textarea` | a field of several lines | `rows`, `monospace`; `on_submit` on Ctrl+Enter (Cmd+Enter on macOS) |
| `number` | a number field, or a slider | `min`, `max`, `step`, `widget = "slider"` |
| `toggle` | a checkbox | |
| `select` | a dropdown | `choices = [{ value, label }]` |
| `multiselect` | checkboxes in a dropdown; the value is a list | `choices` |
| `lang` | Glossa's language picker; the value is a code | `allow_auto` |
| `file` | a path and a *Browse…* button | `filters`; the module gets the path, never the file |
| `button` | an action's button | `action`; `@id` in a layout is the short form |
| `row` | its children side by side | `children` |
| `group` | a titled frame | `children`, `collapsed` |
| `tabs` | one tab per child group | `children` |
| `result` | the bound value, drawn by `kind` | `kind`, `height` (scroll beyond it) |
| `list` | rows of `{ title, subtitle, actions }` | `on_click`, `height` |
| `image` | a base64 PNG or JPEG (a `data:` prefix is fine) | `height` |
| `progress` | a bar for a number from 0 to 1 (or a percentage above 1); `null` spins | |

`result` kinds:

| `kind` | draws |
|---|---|
| `translation` (the default) | text as the popup shows a translation, with copy and listen buttons |
| `article` | a dictionary article as the dictionary window shows one: `{ headword, pronunciations, explanations: [{ part_of_speech, definitions }], examples, associations, link }`, the shape the protocol's [`dictionary`](protocol.md#dictionary) result has |
| `text` | plain text (`monospace = true` for code) |
| `markdown` | a small subset: `#` headings, `-` and `1.` lists, fenced code, `**bold**`, `*italics*`, `` `code` ``; links stay text |
| `table` | a list of lists, or a list of objects (the first one's keys are the header) |
| `json` | any value, pretty-printed |

A list row's `actions` are action ids drawn as small buttons on the row;
like a click on the row, they run with the row as `ev.event.item`. A type
from a newer format shows as a line saying it needs a newer Glossa.

Nothing a module sends is interpreted: text is drawn as text, never as
markup; images are decoded by Glossa; there are no icons in format 1.

## Actions in a panel

An action is one of three kinds:

- **A handler** (`handler`, or the action's id): module code, through the
  protocol's [`action`](protocol.md#action) method. `ev.panel` is the
  panel's id, `ev.state` its state, `ev.event` what set it off: `type` is
  `click`, `submit`, `change`, `open` or `shortcut`, `element` the element's
  id, and `item` a clicked list row.
- **A call** (`call = "translate" | "dictionary" | "detect" | "speak"`): the
  module's own capability through Glossa's pipeline, with `args` giving the
  request (`text`, `from`, `to`, `lang`) and `into = "state.<key>"` where
  the result goes: the translated text, the article, the detected language's
  code. `speak` plays the audio. A streaming translator's text appears in
  `into` as it is written.
- **A built-in**: `builtin = "copy"` (`args.text` to the clipboard),
  `"open_url"` (`args.url`, `http`/`https` only) or `"speak"` (`args.text`
  in Glossa's current voice, `args.lang` optional). `copy` and `open_url`
  run only for the user's own gesture (a click, Enter in a field, a
  shortcut); from `on_open` or `on_change` they fail, and `module check`
  says so.

`args` values are `state.<key>`, `options.<key>`, `item` or `item.<field>`
(the clicked row), with further `.field` steps into objects; anything else
is taken as the text itself.

One action runs at a time per panel; while it runs its button shows a
spinner and the others wait. Closing the panel cancels it and drops its
answer. `needs` works as on the settings page, against the saved settings.
`shortcut = "Ctrl+Enter"` runs an action while its panel has the focus
(Ctrl is Cmd on macOS). A reply's `notice` shows at the foot of the panel,
and its `errors`, keyed by state key, under the element bound to that key.

## Streaming

A slow handler (an LLM explaining a sentence) can show its work as it goes:
each `ui_update` host call merges partial state into the panel at once.

| runtime | call |
|---|---|
| JavaScript | `host.ui.update({ result: "…so far" })` |
| Python | `host.ui_update({"result": "…so far"})` |
| Java | `host.uiUpdate(Map.of("result", "…so far"))` |
| any other | the [`ui_update`](protocol.md#ui_update) host call |

The final reply's `state` still lands last. Updates that come faster than
the window draws are merged, the latest value of each key winning, rather
than queued. Outside a panel the call does nothing.

## Translations

`locales/<lang>.toml` translates a panel like a settings page:

```toml
[panels.glossary]
title = "Глоссарий"

[panels.glossary.items.query]
placeholder = "Поиск по глоссарию"     # also label, help, choices.<value>
```

Action labels come from `[actions.<id>]`, and text the code builds from
`[strings]` through `host.t`.

## Limits

What a module puts into a panel is bounded, so a broken or hostile module
cannot stall the window:

| what | limit |
|---|---|
| a text value | 256 KB, then cut |
| a list (and any array) | 1000 entries |
| an image | 8 MB of base64, 4096 pixels a side and 16 megapixels; a larger one is not shown. Pictures are decoded on a worker, once per change |
| one streamed update | 1 MB as JSON; the largest values are dropped from a bigger one |
| the whole state | 12 MB as JSON; a value that would take it over keeps its old value |
| the kept (`persist`) keys | 1 MB on disk; a bigger file is ignored, and the largest keys are not written |
| state keys | 256 |
| elements in a panel | 500 |
| nesting of containers | 16 |

## From the command line and the control API

```console
$ glossa-cli module panel                       # every panel the loaded modules declare
pocket-glossary/glossary  Pocket Glossary: Glossary  (tray, main_menu)

$ glossa-cli module panel pocket-glossary/glossary                 # its elements and state
$ glossa-cli module panel pocket-glossary/glossary --action word_of_the_day
update progress = 30
update status = "Picking a word…"
…
$ glossa-cli module panel pocket-glossary/glossary --action look_up --item '{"title":"idiom"}'
$ glossa-cli module panel pocket-glossary/glossary --open          # in the running Glossa
```

`--action` runs any kind of action, prints the streamed updates, the state
after it, and what Glossa would do (`would {"copy": "…"}`). The control API
lists panels at `GET /v1/panels` and opens one with `POST /v1/panel`
([control-api.md](../control-api.md#panels)).

## Testing

Fixtures run panel actions with `panel`: the handler gets the panel's id,
its `ui_update` calls come back as the result's `updates`, and `item` is a
clicked row ([testing.md](testing.md#case-fields)).

```toml
[[case]]
name = "word of the day streams its progress"
op = "action"
action = "word_of_the_day"
panel = "glossary"
state = { day = 3 }
[case.expect]
result = { state = { query = "gloss" }, updates = [{ progress = 30 }, { progress = 70 }] }
```

`call` and `builtin` actions need no cases: they run no module code.
`glossa-cli module check` reports mistakes in a panel: unknown types,
bindings to undeclared keys, events naming missing actions, links that are
not web addresses, items placed twice or not at all.
