# Process modules: Java, Node, Go, C#… over stdio

Any program that reads lines from stdin and writes lines to stdout can be a
Glossa module. Glossa starts it on first use, sends it requests as JSON, lets
it call back for HTTP, logging and the cache, and stops it when it has been
idle for a while. Python modules work this way too; the process runtime opens
the same door to every other language.

| `runtime` | what Glossa runs | SDK |
|---|---|---|
| `"process"` | the `command` you give: `["node", "main.mjs"]`, `["./my-module"]`, `["dotnet", "Module.dll"]`… | none needed; the [framing](#framing) is a few dozen lines in any language |
| `"java"` | `java -jar <entry>`, or `java -cp <classpath> <main_class>`, with Java found for you | [`sdk/java`](../../sdk/java/README.md) (Java 17+, no dependencies; Kotlin and other JVM languages use it too) |
| `"python"` | `python <entry>` with the SDK on `PYTHONPATH` | [`sdk/python`](../../sdk/python/README.md) |

All three share one transport inside Glossa, so they behave identically:
same timeouts, same restart after a crash, same logging, and every host call
Glossa offers (including ones added later) reaches them without changes.

## Contents

- [A process module](#a-process-module): [manifest fields](#manifest-fields)
- [Framing](#framing)
- [Lifetime](#lifetime)
- [Java modules](#java-modules): [writing one](#writing-one),
  [Kotlin](#kotlin)
- [Debugging](#debugging)

## A process module

```
modules/example-node/
├── module.toml
└── main.mjs
```

```toml
capabilities = ["translate", "detect_language"]

[module]
id = "example-node"
name = "Reverse words (Node.js example)"
runtime = "process"
command = ["node", "main.mjs"]
env = { NODE_NO_WARNINGS = "1" }
idle_timeout_secs = 60

[permissions]
network = []
```

The complete example, the protocol implemented in plain Node.js with no SDK,
is [`modules/process/example-node`](../../modules/process/example-node).

### Manifest fields

All of these sit in `[module]` and are optional except `command`.

| field | meaning |
|---|---|
| `command` | The program and its first arguments: a list (`["node", "main.mjs"]`), a single string (`"./my-module"`), or one per platform (below). |
| `args` | More arguments, appended after `command` (after the script or jar for Python and Java). |
| `env` | Environment variables for the process, on top of Glossa's own: `env = { LANG = "C.UTF-8" }`. |
| `cwd` | Working directory, relative to the module's folder. The default is the module's folder. |
| `idle_timeout_secs` | Stop the process after this long unused (default 300; `0` keeps it running). |

`${MODULE_DIR}` in `command`, `args`, `env` and `cwd` is replaced with the
module's folder, for tools that want absolute paths.

**Finding the program.** A program given with a path (`./my-module`,
`bin/tool`) is looked for relative to the module's folder. A bare name
(`node`, `dotnet`) is looked for on `PATH`. On Windows a name without an
extension also tries `.exe`, `.cmd`, `.bat` and the rest of `PATHEXT`, so
`npx` finds `npx.cmd` and a shim from a version manager (nvm-windows,
pyenv-win, Volta) works; `.cmd` and `.bat` files run through `cmd.exe /d /c`
with proper argument escaping. `java` at the head of a command gets the Java
search described [below](#java-modules).

A program that cannot be found fails the module's load with a message saying
so; `glossa-cli modules` and *Options → Modules* show it.

**One command per platform.** Use dotted keys:

```toml
[module]
runtime = "process"
command.windows = ["bin/my-module.exe"]
command.macos = ["bin/my-module-macos"]
command.linux = ["bin/my-module-linux"]
# command.unix covers Linux, macOS and the BSDs when their own key is absent;
# command.default covers anything left.
```

A module with no command for the running platform fails to load, saying
which platform it lacks.

## Framing

One JSON object per line, UTF-8, in both directions. Nothing else: no
handshake, no headers, no length prefixes.

1. Glossa writes a request:
   `{"id": 7, "method": "translate", "params": {"text": "Hello", "from": "auto", "to": "ru", "options": {}}}`
2. The module may call back while it works, with a **string** id of its own:
   `{"id": "h1", "method": "http", "params": {"method": "GET", "url": "https://api.example.com/x"}}`
   Glossa answers on stdin: `{"id": "h1", "result": {"status": 200, "headers": {…}, "body": "…"}}`
   or `{"id": "h1", "error": {"kind": "network", "message": "…"}}`.
3. The module answers the request with its number:
   `{"id": 7, "result": {"text": "Привет"}}` or
   `{"id": 7, "error": {"kind": "missing_credential", "message": "api_key"}}`.

Requests come one at a time: Glossa never sends a second request before the
first is answered, so a module needs no concurrency. Methods, results, error
kinds and host calls are in the [protocol reference](protocol.md); a method
the module does not implement should answer
`{"kind": "unsupported", …}`.

Rules that keep it robust:

- **Lines Glossa cannot read are logged, not fatal.** Anything on stdout that
  is not a JSON object with an `id` goes to the log, so a stray debug print
  does not break the module. Still, print debugging output to **stderr**:
  Glossa logs every stderr line under the module's id.
- **Flush after every line.** Many runtimes buffer stdout when it is a pipe
  (C's stdio, Go's `bufio`, .NET's `Console.Out` with autoflush off). A reply
  that sits in a buffer looks like a hang and ends in a timeout.
- **Write UTF-8 and no BOM.** Glossa sets no locale for the process; set the
  output encoding explicitly where the runtime has one
  (`Console.OutputEncoding = new UTF8Encoding(false)` in .NET).
- **Escape newlines inside strings** (any JSON encoder does), so one message
  stays one line.
- **Keep running until stdin closes.** End of input means Glossa is done
  with the process; exit then.

Glossa also sets `GLOSSA_MODULE_ID` and `GLOSSA_PROTOCOL` (the protocol
version, `1`) in the module's environment, and for a module with
[assets](README.md#assets) `GLOSSA_ASSETS_DIR` and one
`GLOSSA_ASSET_<NAME>` per file.

## Lifetime

- The process starts on the first call, not when the module loads: an
  installed module costs nothing until it is used.
- It is stopped after `idle_timeout_secs` without calls, when the settings
  are applied or modules reloaded, and when Glossa exits. The next call
  starts it again, so keep anything that must survive in the host cache
  (`cache_set` / `cache_get`), not in memory.
- A call has the time budget of *Options → Modules → Call timeout*
  (`[modules] call_timeout_ms`, 30 s by default); the clock stops while
  Glossa works on a host call. A module that overruns it is killed, and the
  next call starts a fresh process.
- A process that exits on its own is reported with its exit code and the
  last lines of its stderr, then started again on the next call.

## Java modules

```toml
capabilities = ["translate", "ocr"]

[module]
id = "example-upper"
name = "UPPER (Java example)"
runtime = "java"
entry = "module.jar"          # the default
jvm_args = ["-Xmx64m"]
idle_timeout_secs = 60
```

| field | meaning |
|---|---|
| `entry` | The jar to run with `java -jar` (default `module.jar`). It must exist when the module loads. |
| `main_class` | Run this class instead of a jar's `Main-Class`. |
| `classpath` | With `main_class`: classpath entries relative to the module's folder (`["classes", "lib/*"]`). Without one, `entry` (if set) or the folder itself. |
| `jvm_args` | JVM options, placed before `-jar` / the class: `-Xmx`, `-D…`, `--enable-preview`. |
| `args`, `env`, `cwd`, `idle_timeout_secs` | As for process modules. |

Glossa adds `-XX:+UseSerialGC` (the lightest collector for a small heap) and
`-Djava.awt.headless=true` (so image code never opens a window or a Dock
icon) before your `jvm_args`, which can override them.

**Finding Java.** In order: the *Java* field in *Options → Modules*
(`[modules] java` in `config.toml`: a `java` executable or a Java home
folder), then `$JAVA_HOME/bin/java`, then `java` on `PATH`, then the folders
installers use (`Program Files\Eclipse Adoptium`, `…\Java`, `…\Microsoft`,
`…\Zulu`, `…\BellSoft`, `…\Amazon Corretto` on Windows;
`/Library/Java/JavaVirtualMachines` on macOS; `/usr/lib/jvm` and SDKMAN on
Linux). When there is none, the module fails to load with instructions:

```
$ glossa-cli modules
failed (1):
  …/modules/example-upper: Java not found. Install a Java 17 or newer runtime, for example
  Eclipse Temurin (https://adoptium.net; `winget install EclipseAdoptium.Temurin.21.JRE` on
  Windows, `sudo apt install openjdk-17-jre-headless` on Debian or Ubuntu,
  `brew install --cask temurin` on macOS), then set JAVA_HOME or put java on PATH, or give the
  path to java in Options > Modules
```

A JRE is enough to run a module; building one needs a JDK.

### Writing one

```java
import io.github.bropines.glossa.*;

public final class Main implements GlossaModule {
    @Override
    public Object translate(Request req, Host host) throws Exception {
        String key = req.requireOption("api_key");          // missing_credential if empty
        Object data = host.postJson("https://api.example.com/translate",
                Json.obj("q", req.text(), "target", req.to()),
                java.util.Map.of("Authorization", "Bearer " + key)).json();
        return Json.obj("text", Json.string(data, "translation", ""));
    }

    public static void main(String[] args) {
        Glossa.run(new Main());
    }
}
```

Build it into one jar together with the SDK's sources, with plain `javac` and
`jar`; [`modules/java/example-upper`](../../modules/java/example-upper) has a
`build.sh` and a `build.ps1` that do exactly that. The SDK, its API and the
optional Gradle build are described in
[`sdk/java/README.md`](../../sdk/java/README.md).

JVM start-up takes a fraction of a second and a small module's process sits
at a few tens of MB, which is why it starts on first use and stops when idle.
Keep `jvm_args = ["-Xmx64m"]` or similar unless the module needs a big heap.

### Kotlin

Kotlin (and Scala, Groovy, Clojure) compiles to the same bytecode and uses the
Java SDK as is. No Kotlin support is needed in Glossa; the module is a jar like
any other.

```kotlin
import io.github.bropines.glossa.*

class Shout : GlossaModule {
    override fun translate(req: Request, host: Host): Any {
        host.debug("shouting ${req.text().length} characters")
        return Json.obj("text", req.text().uppercase(), "detected_from", "en")
    }
}

fun main() = Glossa.run(Shout())
```

Compile against the SDK's jar, with the Kotlin runtime included, and put
both jars in the module's folder:

```bash
sdk/java/build.sh                                   # -> sdk/java/build/glossa-module-sdk.jar
cp sdk/java/build/glossa-module-sdk.jar modules/shout/
kotlinc modules/shout/Shout.kt -cp modules/shout/glossa-module-sdk.jar \
    -include-runtime -d modules/shout/shout.jar
```

```toml
[module]
id = "shout"
name = "Shout (Kotlin)"
runtime = "java"
main_class = "ShoutKt"                              # Kotlin's class for a top-level main()
classpath = ["shout.jar", "glossa-module-sdk.jar"]
```

With Gradle, the usual `application` or shadow-jar setup works the same way:
one runnable jar with the SDK inside, named in `entry`.

## Debugging

- `glossa-cli modules` lists every module with its runtime, and the ones that
  failed to load with the reason (program not found, jar missing, no Java).
- `glossa-cli -v translate -s my-module -t de "Hi"` shows the module's stderr
  and its `log` host calls as they happen; the same lines go to
  `<cache folder>/glossa.log`. A crash ends with the exit code and the last
  stderr lines.
- Run the module by hand and type at it. A request needs nothing but a line:

  ```bash
  cd modules/my-module
  echo '{"id":1,"method":"translate","params":{"text":"hi","from":"en","to":"de","options":{}}}' | java -jar module.jar
  ```

  A module that makes host calls prints `{"id":"h1",…}` and waits; answer it
  by typing `{"id":"h1","result":null}`.
- `GLOSSA_HOME=<dir> glossa-cli …` gives a clean data folder with only your
  module in `<dir>/modules`.
- "UnsupportedClassVersionError" in the log means the jar was built for a
  newer Java than the one Glossa found; set the Java path in *Options →
  Modules* or rebuild with `--release 17`.
- A call that always times out usually means stdout is buffered: flush after
  each line.
