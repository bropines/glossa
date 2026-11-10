# Documentation

Everything about Glossa beyond the [README](../README.md), by topic.
[README.ru.md](README.ru.md) is the same index in Russian, with the pages
that exist in Russian.

## Starting out

| page | what it covers |
|---|---|
| [Getting started](getting-started.md) | Installing, the first start and the welcome guide, the first translation, the default hotkeys. |
| [User guide](guide.md) | Task by task: translating a selection, the popup, the main window, services, the dictionary, history, text on the screen, listening, presets, mouse mode, exceptions, hotkeys, adding an AI translator, installing a module. |
| [Installing](install.md) | System requirements; which download to take; the Windows installer, macOS disk image, `.deb`, AppImage and portable archives; first launch of the unsigned beta, where files go, upgrading, removing completely; accessibility; building the packages. |
| [Troubleshooting](troubleshooting.md) | By symptom: the program does not start, a hotkey does nothing, the popup is empty, macOS permissions, elevated windows on Windows, Wayland, proxy and certificate errors, rate limiting, text recognition, module approval and downloads, the log, what to attach to a bug report. |

## Using Glossa

| page | what it covers |
|---|---|
| [Text recognition](ocr.md) | OCR engines (Google Lens, Windows OCR, OneOCR, Apple Vision, Tesseract, AI vision models, commands) and setting them up; the OCR actions, the result window, copy mode, Lens's own translation, translating in place. |
| [AI endpoints](ai-endpoints.md) | OpenAI-compatible translators: presets, settings, streaming, troubleshooting. |
| [Themes](theming.md) | Theme files, light and dark themes that follow the system, backdrops, transparency, classic themes. |
| [Configuration](configuration.md) | `config.toml` field by field, where it lives, the other files in the data folder, and keys in the system keychain. |
| [Command line](cli.md) | Every `glossa-cli` command and flag, output and exit codes, environment variables. |
| [Control API](control-api.md) | The local HTTP API (and D-Bus on Linux) a running Glossa answers: endpoints, the token, binding hotkeys on Wayland, macOS and with AutoHotkey. |

## Privacy, security and administration

| page | what it covers |
|---|---|
| [Privacy and data](privacy.md) | What leaves the machine, per feature and host; that there is no telemetry and no update check; fallbacks that can send text to another service; what is stored and where; working offline; clearing data. |
| [Deployment](deployment.md) | For administrators: unattended installation on each system, where files go, preparing `config.toml` for users, `GLOSSA_HOME` and portable mode, proxy and certificates, the settings that reduce risk, the hosts to allow by feature, removal. There is no policy layer yet. |
| [SECURITY.md](../SECURITY.md) | Reporting a vulnerability, supported versions, and what Glossa does and does not protect: module approvals, module permissions, the control API, secrets. |

## Writing modules

| page | what it covers |
|---|---|
| [Modules overview](modules/README.md) | What a module is, where modules live, `module.toml`, settings pages and actions, assets, Python dependencies, permissions and trust, the bundled modules. |
| [JavaScript modules](modules/js-modules.md) | Modules run in process. |
| [Python modules](../sdk/python/README.md) | Modules run as a child process, with the Python SDK. |
| [Java modules](../sdk/java/README.md) | Modules in Java, Kotlin or any JVM language, with the Java SDK. |
| [Native modules](modules/native-modules.md) | Shared libraries in Rust, C or anything with a C ABI. |
| [Process modules](modules/process-modules.md) | Modules that run as a separate program, in any language. |
| [OCR modules](modules/ocr-modules.md) | Text recognition engines as modules. |
| [Module panels](modules/panels.md) | Windows of a module's own, described in `module.toml` and drawn by Glossa. |
| [The module editor](modules/editor.md) | Writing, checking, reloading and testing a module inside Glossa. |
| [Testing a module](modules/testing.md) | `glossa-cli module test` and `module check`: fixtures, recorded HTTP answers, the protocol checks. |
| [Protocol](modules/protocol.md) | The JSON requests and host calls every runtime shares. |
| [Classic scripts](modules/classic-scripts.md) | QTranslate `Service.js` scripts and pot-app plugins. |
| [AGENTS.md for your module](modules/agents-template.md) | A ready text for a module's own repository, for AI coding assistants. |

## Inside Glossa

| page | what it covers |
|---|---|
| [Architecture](architecture.md) | The crates and how a translation flows through them, with a diagram. |
| [RFC: declarative modules](design/declarative-modules.md) | The proposal that module settings pages, actions, panels and the module editor grew from. For what was built, read the module pages above. |
| [Module runtimes FAQ](design/runtimes-faq.md) | Is Python embedded? Is JavaScript? What embedding Python would take. |
| [How the classic program behaved](research/classic-behaviour.md) | Clean-room notes on QTranslate's observed behaviour. |
| [Google Translate without a key](research/google-translate.md) | What the built-in Google services call, how Google limits them, and how Glossa copes. |
| [Local engines](research/local-engines.md) | Which offline engines are worth offering as modules: size, speed, quality, licences. |
| [Fluent numbers](research/fluent.md) | Where the Fluent themes' sizes and colours come from. |

## The project

| page | what it covers |
|---|---|
| [SUPPORT.md](../SUPPORT.md) | Where to ask, and what to include. English and Russian are welcome. |
| [CONTRIBUTING.md](../CONTRIBUTING.md) | Building and sending changes. |
| [AGENTS.md](../AGENTS.md) | Rules for changing the repository, human or agent. |
| [CODE_OF_CONDUCT.md](../CODE_OF_CONDUCT.md) | How people here treat each other. |
| [RELEASING.md](../RELEASING.md) | The release checklist: versions, notes, tags, what to check. |
| [CHANGELOG.md](../CHANGELOG.md) | What changed in each release. |
