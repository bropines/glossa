# Documentation

Everything about Glossa beyond the [README](../README.md), by topic.
[README.ru.md](README.ru.md) is the same index in Russian.

## Using Glossa

| page | what it covers |
|---|---|
| [Installing](install.md) | Which download to take; the Windows installer, macOS disk image, `.deb`, AppImage and portable archives; first launch of the unsigned beta, where files go, upgrading, removing; building the packages. |
| [Command line](cli.md) | Every `glossa-cli` command and flag, output and exit codes, environment variables. |
| [Control API](control-api.md) | The local HTTP API (and D-Bus on Linux) a running Glossa answers: endpoints, the token, binding hotkeys on Wayland, macOS and with AutoHotkey. |
| [Configuration](configuration.md) | `config.toml` field by field, where it lives, the other files in the data folder, and keys in the system keychain. |
| [Text recognition](ocr.md) | OCR engines (Google Lens, Windows OCR, OneOCR, Apple Vision, Tesseract, AI vision models, commands) and setting them up; the OCR actions, the result window, copy mode, Lens's own translation, translating in place. |
| [AI endpoints](ai-endpoints.md) | OpenAI-compatible translators: presets, settings, streaming, troubleshooting. |
| [Themes](theming.md) | Theme files, light and dark themes that follow the system, backdrops, transparency, classic themes. |

## Writing modules

| page | what it covers |
|---|---|
| [Modules overview](modules/README.md) | What a module is, where modules live, `module.toml`, the bundled modules. |
| [JavaScript modules](modules/js-modules.md) | Modules run in process. |
| [Python modules](../sdk/python/README.md) | Modules run as a child process, with the Python SDK. |
| [Native modules](modules/native-modules.md) | Shared libraries in Rust, C or anything with a C ABI. |
| [OCR modules](modules/ocr-modules.md) | Text recognition engines as modules. |
| [Process modules](modules/process-modules.md) | Modules that run as a separate program. |
| [Protocol](modules/protocol.md) | The JSON requests and host calls every runtime shares. |
| [Classic scripts](modules/classic-scripts.md) | QTranslate `Service.js` scripts and pot-app plugins. |
| [AGENTS.md for your module](modules/agents-template.md) | A ready text for a module's own repository, for AI coding assistants. |

## Inside Glossa

| page | what it covers |
|---|---|
| [Architecture](architecture.md) | The crates and how a translation flows through them, with a diagram. |
| [How the classic program behaved](research/classic-behaviour.md) | Clean-room notes on QTranslate's observed behaviour. |
| [Fluent numbers](research/fluent.md) | Where the Fluent themes' sizes and colours come from. |
| [AGENTS.md](../AGENTS.md) | Rules for changing the repository, human or agent. |
| [CONTRIBUTING.md](../CONTRIBUTING.md) | Building and sending changes. |
| [CHANGELOG.md](../CHANGELOG.md) | What changed in each release. |
