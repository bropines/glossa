# Build the Glossa module SDK for Java with nothing but a JDK (17 or newer).
#
#   .\build.ps1          -> build\glossa-module-sdk.jar
#   .\build.ps1 test     -> the jar, then the SDK's own tests
#
# Uses $env:JAVA_HOME\bin when JAVA_HOME is set, else javac/jar/java from PATH.
param([string]$Task = "")
$ErrorActionPreference = "Stop"
$here = $PSScriptRoot
$out = Join-Path $here "build"
$bin = if ($env:JAVA_HOME) { Join-Path $env:JAVA_HOME "bin" } else { "" }
function Tool($name) { if ($bin) { Join-Path $bin "$name.exe" } else { $name } }
function Check { if ($LASTEXITCODE -ne 0) { throw "$args failed with exit code $LASTEXITCODE" } }

Remove-Item -Recurse -Force (Join-Path $out "classes"), (Join-Path $out "test-classes") -ErrorAction SilentlyContinue
New-Item -ItemType Directory -Force (Join-Path $out "classes") | Out-Null
# An argument file keeps long source lists and paths with spaces working;
# javac reads backslashes in quoted entries as escapes, so use forward slashes.
Get-ChildItem -Recurse -Filter *.java (Join-Path $here "src") |
    ForEach-Object { '"' + ($_.FullName -replace '\\', '/') + '"' } |
    Set-Content -Encoding ascii (Join-Path $out "sources.txt")
& (Tool javac) --release 17 -encoding UTF-8 -Xlint:all -d (Join-Path $out "classes") "@$(Join-Path $out 'sources.txt')"; Check javac
& (Tool jar) --create --file (Join-Path $out "glossa-module-sdk.jar") -C (Join-Path $out "classes") .; Check jar
Write-Host "built $(Join-Path $out 'glossa-module-sdk.jar')"

if ($Task -eq "test") {
    New-Item -ItemType Directory -Force (Join-Path $out "test-classes") | Out-Null
    Get-ChildItem -Recurse -Filter *.java (Join-Path $here "test") |
        ForEach-Object { '"' + ($_.FullName -replace '\\', '/') + '"' } |
        Set-Content -Encoding ascii (Join-Path $out "test-sources.txt")
    & (Tool javac) --release 17 -encoding UTF-8 -cp (Join-Path $out "classes") -d (Join-Path $out "test-classes") "@$(Join-Path $out 'test-sources.txt')"; Check javac
    & (Tool java) -cp "$(Join-Path $out 'classes');$(Join-Path $out 'test-classes')" io.github.bropines.glossa.SdkTest; Check java
}
