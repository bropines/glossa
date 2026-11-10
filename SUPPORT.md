# Getting help

Glossa is a free, open-source project maintained in spare time. There is
no paid support and no guaranteed response time. Вопросы и сообщения об
ошибках можно писать по-русски.

## Before asking

1. [Troubleshooting](docs/troubleshooting.md) covers the usual problems by
   symptom (по-русски: [docs/ru/troubleshooting.md](docs/ru/troubleshooting.md)).
2. `glossa-cli doctor` says what works on your machine and why something
   does not.
3. The [documentation index](docs/README.md) lists every page
   ([по-русски](docs/README.ru.md)).
4. Search the [existing issues](https://github.com/bropines/glossa/issues?q=is%3Aissue),
   closed ones included.

## Where to ask

| what | where |
|---|---|
| A bug, or something that does not work as documented | [a bug report](https://github.com/bropines/glossa/issues/new?template=bug_report.yml) |
| An idea or a missing feature | [a feature request](https://github.com/bropines/glossa/issues/new?template=feature_request.yml) |
| A question about using Glossa that the documentation does not answer | a bug report as well: an unanswered question is a gap in the documentation |
| A security problem | privately, as [SECURITY.md](SECURITY.md) describes; never in a public issue |
| A change you want to make yourself | [CONTRIBUTING.md](CONTRIBUTING.md) |

The issue tracker is the only support channel. There is no forum, chat or
support e-mail address.

English and Russian are both welcome: write in whichever you are more
comfortable with.

## What to include

A report that can be acted on at once has:

- what you did, what you expected and what happened, with the exact text
  of any message;
- the Glossa version (*About Glossa* in the tray menu, or `glossa-cli
  --version`);
- the output of `glossa-cli doctor` (the system, what works in your
  session, the folders, the proxy);
- the lines of the log around the problem
  ([where the log is](docs/troubleshooting.md#the-log));
- for a service or module: which one, and the output of `glossa-cli
  modules`.

**Never post API keys, tokens or passwords.** Look through the log and
any piece of `config.toml` before pasting and replace secrets with `***`.

## What to expect

- Windows problems can usually be reproduced. macOS is in early testing.
  Linux has not been tried on real hardware by the maintainer, so Linux
  reports are especially useful and may take longer to resolve.
- Keyless services break when the websites behind them change. Reports
  about a service that stopped working are welcome; the fix comes in a
  later release, or sooner as your own copy of the module
  ([modules](docs/modules/README.md)).
- Only the latest release is supported. Please check that the problem is
  still there in it.

Everyone taking part is expected to follow the
[Code of Conduct](CODE_OF_CONDUCT.md).
