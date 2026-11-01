# Glossa module SDK for Java

Write Glossa modules (translators, dictionaries, voices, OCR engines) in Java,
Kotlin or any JVM language. Java 17 or newer, no dependencies, six small
source files in the package `io.github.bropines.glossa`. Glossa runs the module
as a child process and talks to it over stdin and stdout; how that works, the
manifest fields and debugging tips are in
[docs/modules/process-modules.md](../../docs/modules/process-modules.md).

## A module

```java
import io.github.bropines.glossa.*;
import java.util.Locale;

public final class Upper implements GlossaModule {
    @Override
    public Object translate(Request req, Host host) throws Exception {
        host.debug("uppercasing " + req.text().length() + " characters");
        return Json.obj("text", req.text().toUpperCase(Locale.ROOT), "detected_from", "en");
    }

    public static void main(String[] args) {
        Glossa.run(new Upper());
    }
}
```

```toml
# module.toml
capabilities = ["translate"]

[module]
id = "upper"
name = "UPPER"
runtime = "java"
entry = "module.jar"
```

The complete example, with build scripts, is
[`modules/java/example-upper`](../../modules/java/example-upper).

## API

**`GlossaModule`**: implement what your manifest's `capabilities` promise;
the rest answer "unsupported".

| method | capability | request | return |
|---|---|---|---|
| `translate(req, host)` | `translate` | `text()`, `from()`, `to()`, `raw().get("glossary")` | `{text, detected_from?, romanization?, alternatives?}` or a `String` |
| `detect(req, host)` | `detect_language` | `text()` | `{lang, confidence?}` or the code as a `String` |
| `dictionary(req, host)` | `dictionary` | `text()`, `from()`, `to()`, `bool("exact", false)` | an article: `{headword, pronunciations, explanations, examples, …}` |
| `speak(req, host)` | `listen` | `text()`, `lang()`, `bool("slow", false)` | `{audio_base64, mime}`; a `byte[]` is written as base64 |
| `recognize(req, host)` | `ocr` | `image()` (bytes), `lang()`, `translateTo()` | `{text, translation?, detected_lang?}` or a `String` |
| `handle(req, host)` | | `req.method()` | for methods newer than this SDK |

Results are anything `Json.write` takes: `Json.obj("text", t)` maps, any
`Map` or `List`, or records, which become objects with their component names:

```java
record Translation(String text, String detected_from) {}
return new Translation(out, "en");
```

The field names and shapes are the [protocol's](../../docs/modules/protocol.md).

**`Request`**: `text()`, `from()`, `to()`, `lang()`, `image()`,
`translateTo()`, `options()`, `option(key, fallback)`, `requireOption(key)`
(throws `missing_credential` naming the setting, so Glossa tells the user
what to fill in), `string(key, fallback)`, `bool(key, fallback)`, and `raw()`
for everything.

**`Host`**: calls back into Glossa. Each blocks until Glossa answers and
throws `ModuleError` with Glossa's error kind when it fails.

| call | does |
|---|---|
| `fetch(url)`, `fetch(method, url, headers, body)` | HTTP through Glossa: the user's proxy and timeouts, only hosts in `[permissions] network`. Returns `HttpResponse`: `status()`, `headers()`, `header(name)`, `body()`, `bytes()`, `ok()`, `json()` (2xx required; 429 becomes `rate_limited`). |
| `postJson(url, value, headers)` | POST a JSON body. |
| `http(map)` | the `http` host call with every protocol field (`body_base64`, `timeout_ms`, `binary`). |
| `log(level, msg)`, `debug`, `info`, `warn` | Glossa's log, tagged with the module id. |
| `cacheGet(key)`, `cacheSet(key, value, ttlSecs)` | a small store in Glossa's memory that outlives this process (which stops when idle). |
| `call(method, params)` | **any host call by name**, returning the parsed result. Host calls added to Glossa after this SDK work through it before they get a typed method. |

**`ModuleError(kind, message)`**: a failure Glossa can explain. Kinds:
`missing_credential` (message = the option key), `rate_limited` (with
`retryAfterSecs`), `not_found`, `unsupported`, `bad_request`,
`bad_response`, `http`, `network`, `source_language`, `target_language`,
`other`. Any other exception becomes `other` with its stack trace, and the
module keeps running.

**`Json`**: `parse(String)` gives `Map` (in document order), `List`,
`String`, `Long` / `Double`, `Boolean` and `null`; `write(Object)`;
`obj(k, v, …)`, `asMap`, `asList`, `string(obj, key, fallback)`.

**`Glossa.run(module)`** serves requests until Glossa closes stdin, then
exits. It points `System.out` at stderr first, so `System.out.println`
lands in Glossa's log instead of corrupting the protocol (native code
writing to file descriptor 1 directly still reaches the pipe; Glossa logs and
skips such lines). `Glossa.serve(module, in, out)` runs the same loop on any
streams, for tests.

Calls arrive one at a time on the main thread, so fields need no locks. Make
host calls from that thread, while handling the request.

## Building

Plain `javac` and `jar`, no build tool needed:

```bash
./build.sh            # build/glossa-module-sdk.jar
./build.sh test       # and run the SDK's tests
```

```powershell
.\build.ps1
.\build.ps1 test
```

Both use `$JAVA_HOME/bin` when `JAVA_HOME` is set, else the tools on `PATH`.
`build.gradle.kts` is there for projects that already use Gradle; nothing
needs it.

A module is simplest to ship as one jar holding its own classes and the
SDK's: compile both source trees together and set the `Main-Class`, as
[`modules/java/example-upper/build.sh`](../../modules/java/example-upper/build.sh)
does:

```bash
javac --release 17 -encoding UTF-8 -d build/classes $(find src ../../sdk/java/src -name '*.java')
jar --create --file module.jar --main-class example.ExampleUpper -C build/classes .
```

Or keep the SDK as a separate jar and list both in the manifest
(`main_class` + `classpath`).

## Tests

`test/.../SdkTest.java` covers the JSON reader and writer and the protocol
loop (requests, host calls, errors) with no test framework. `./build.sh test`
runs it, and so does `cargo test -p glossa-modules --test process_modules`
whenever a JDK is around, together with the example module and a check of
every host call against Glossa's real host API.
