# The module editor

Glossa has an editor for modules built in: write a module from a template,
see what is wrong with it as you go, reload it and run its tests, all
without leaving the program or restarting it.

## Opening it

*Options → Modules → Module editor*:

- **New module** — pick a template (translator, dictionary, text
  recognition), give it a name and press *Create and edit*. The module is
  written to your modules folder, loaded, and opened. Each template passes
  its own tests from the start, offline, against recorded answers.
- **Edit** — on one of your own modules (those in your modules folder).
  Folders there that failed to load are listed too, so you can fix them.
- **Copy to my modules and edit** — on a bundled module or one installed
  elsewhere (beside the program, system-wide, a classic installation). The
  module is copied to your modules folder, where the copy replaces the
  original by its id, and the copy is opened. Delete the copy to get the
  original back.

## The window

- **Files** on the left: the module folder's files. Text files open in a
  tab; binaries (a `.jar`, a library, a picture) are listed but not opened.
  Build folders (`target`, `node_modules`, `__pycache__`…) are left out.
- **The editor** in the middle, one tab per file, with colours for
  JavaScript, TOML, JSON and Python, line numbers, and the problems of the
  file underlined with a mark beside the line (hover the mark for the
  message).
- **Preview** on the right: the module's settings page as *Options →
  Services* will show it, from the manifest as last checked. Values typed
  there are not saved. On a narrow window the preview is a tab in the
  drawer instead.
- **The drawer** at the bottom:
  - *Problems* — everything the last check found, and the places failed
    test cases point at, each with `file:line:col`; click one to go there.
  - *Log* — what the module logs while the editor is open: `host.log` and
    `console.*` in JavaScript, `host.log` in Python and the SDKs, a process
    module's stderr. Lines from test runs land here too.
  - *Test results* — each case with pass or fail, how long it took, what
    came back, why it failed and the HTTP requests it made.

| key | does |
|---|---|
| Ctrl+S (Cmd+S on macOS) | save every changed file, check, and reload the module if the check found no errors |
| F5 | save, then run the module's tests |
| Ctrl+F | find in the file (Enter for the next match, Shift+Enter for the previous one) |
| Ctrl+G | go to a line |
| Ctrl+/ | comment or uncomment the selected lines |
| Tab, Shift+Tab | indent or unindent with spaces |
| Esc | close the find or go-to bar |

Enter keeps the line's indentation (and adds a level after `{`, `[`, `(`, or
`:` in Python). Undo and redo are the usual keys.

## Save, check, reload, test

**Save** writes each changed file to a temporary name beside it and then
renames it over the old one, so a crash never leaves half a file and a
running Glossa never loads one.

**Check** looks at the folder without running anything:

1. `module.toml` as TOML, with the line and column of a syntax error;
2. what the manifest says, with the checks of `glossa-cli module check`
   (unknown keys, capabilities, options, permissions, language codes, the
   entry file, hard-coded keys), placed at the key they are about;
3. a JavaScript module's script, parsed by QuickJS, with the position of a
   syntax error;
4. every other TOML and JSON file in the folder (test fixtures, recorded
   answers).

**Reload** loads only this module again and puts it in place of the
provider with its id; every other service stays as it is. If the module no
longer loads, the old one keeps running and the reason is shown. Process
modules (Python, Java, any program) have their process stopped, and the
next call starts the new code. A native library cannot be unloaded safely:
the editor says so, and the new one loads when Glossa starts again.

**Test** runs the cases in the module's `tests/` folder through the same
harness as `glossa-cli module test`, offline, against the recorded answers
([testing.md](testing.md)).

An error thrown while a JavaScript module runs carries the place it was
thrown (the protocol's optional error `location`, see
[protocol.md](protocol.md#errors)), so a failed case links to the line.

Closing the window with unsaved files asks first.

## Outside the window

The same steps are in the engine, so other tools can use them:

- `glossa-cli module check DIR` and `glossa-cli module test DIR` run the
  checks and the tests ([cli.md](../cli.md));
- the control API has `POST /v1/modules/<id>/check`, `…/reload` and
  `…/test` ([control-api.md](../control-api.md#post-v1modules)), for an
  editor of your own that saves the files and asks the running Glossa to
  reload one module.
