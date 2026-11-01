# Build module.jar: this module's sources and the Java SDK's, compiled
# together into one runnable jar (java -jar module.jar). Needs a JDK 17+.
#
# $env:GLOSSA_JAVA_SDK points at the SDK's folder (the one holding src\) when
# this module lives outside the Glossa repository.
$ErrorActionPreference = "Stop"
$here = $PSScriptRoot
$sdk = if ($env:GLOSSA_JAVA_SDK) { $env:GLOSSA_JAVA_SDK } else { Join-Path $here "..\..\..\sdk\java" }
$bin = if ($env:JAVA_HOME) { Join-Path $env:JAVA_HOME "bin" } else { "" }
function Tool($name) { if ($bin) { Join-Path $bin "$name.exe" } else { $name } }
if (-not (Test-Path (Join-Path $sdk "src"))) { throw "the Java SDK was not found at $sdk; set GLOSSA_JAVA_SDK" }

$build = Join-Path $here "build"
Remove-Item -Recurse -Force $build -ErrorAction SilentlyContinue
New-Item -ItemType Directory -Force (Join-Path $build "classes") | Out-Null
Get-ChildItem -Recurse -Filter *.java (Join-Path $here "src"), (Join-Path $sdk "src") |
    ForEach-Object { '"' + ($_.FullName -replace '\\', '/') + '"' } |
    Set-Content -Encoding ascii (Join-Path $build "sources.txt")
& (Tool javac) --release 17 -encoding UTF-8 -d (Join-Path $build "classes") "@$(Join-Path $build 'sources.txt')"
if ($LASTEXITCODE -ne 0) { throw "javac failed" }
& (Tool jar) --create --file (Join-Path $here "module.jar") --main-class example.ExampleUpper -C (Join-Path $build "classes") .
if ($LASTEXITCODE -ne 0) { throw "jar failed" }
Write-Host "built $(Join-Path $here 'module.jar')"
