# Classic QTranslate services and pot-app plugins

Glossa runs two families of scripts written for other programs, unchanged.
Neither is shipped with Glossa: they are loaded from where you already have
them.

## Classic QTranslate services

A QTranslate installation keeps one folder per service under `Services/`,
each with a `Service.js` (and sometimes an icon), plus a shared
`Services/Common.js`. Glossa reads them in place.

**Point Glossa at the installation**, either way:

- *Options → Modules → Classic QTranslate installation*: pick the folder that
  holds `QTranslate.exe`. Tick what to use from it: services, themes, interface
  languages.
- or once, from the command line, which also imports the settings
  (`Data/Options.json`: hotkeys, languages, enabled services, popup and
  appearance options, proxy) and the history (`Data/History.json`):

```bash
glossa-cli import-classic "C:\Program Files (x86)\QTranslate"
```

The services then appear as `qt-<folder name>` in *Options → Services* and in
`glossa-cli services`. Glossa runs them in its JavaScript engine with:

- its own implementation of the helper library the scripts call (`Common.js`
  functions such as request building, encoding and language lookups);
- the `RequestData` / `ResponseData` flow, including multi-step requests
  (`nextRequestHandler`);
- emulations of the Windows Script Host objects some scripts create
  (`ActiveXObject("WinHttp.WinHttpRequest.5.1")`, `MSXML2.XMLHTTP`);
- the session values the classic program scraped for the Microsoft and Google
  scripts, fetched the same way;
- the script's `SupportedLanguages` table mapped onto Glossa's languages, and
  legacy code pages (cp1251, latin-1) for old services.

Classic scripts always had unrestricted network access, so they keep it.
Their settings (keys some services ask for) appear on their settings page
like any module's.

**Themes** (`Themes/*.json`) show up in *Options → Appearance* next to the
built-in ones and can be extended by a Glossa theme (see
[../theming.md](../theming.md)). **Interface languages**
(`Locales/<Language>/lang.json`) show up in *Options → Basics*.

## pot-app plugins

pot-app plugins come as a folder with `info.json` and `main.js`, or packed as
a `.potext` file. Put either into a modules folder, or use *Options → Modules
→ Install module…*; a `.potext` is unpacked into Glossa's cache on load.

Translator, OCR (`recognize`) and text-to-speech (`tts`) plugins work. Glossa
provides what they expect from pot's runtime:

- `utils.http`: the Tauri-flavoured `fetch` with `Body.json/form/text/bytes`
  and `ResponseType`, routed through Glossa's HTTP client and the user's
  proxy;
- `utils.CryptoJS`: the hashes, HMACs and encoders plugins use;
- `utils.readTextFile` / `readBinaryFile` for the plugin's own files, and
  `utils.osType`;
- `setResult` for streaming translators (the last value wins), and `detect`;
- the settings declared in `info.json` (`needs`), shown on the plugin's
  settings page and passed as `config`;
- `info.json`'s language table, mapped onto Glossa's languages.

Not provided: `utils.run` (running programs) and `utils.Database`; a plugin
that needs them fails with a message saying so.

The plugin id becomes `pot-<id>`. pot plugins have no permission list, so
they may reach any host, as they did in pot.

## When something does not work

```bash
glossa-cli modules        # failed scripts are listed with the reason
glossa-cli -v translate -s qt-google -t ru "test"
```

Most failures are the service itself having changed (a new endpoint, a new
token scheme). Those affected the classic program in the same way; the fix is
in the script, or in a Glossa module that replaces it. See
[js-modules.md](js-modules.md#porting).
