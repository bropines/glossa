// Optional: the SDK builds with plain javac and jar (build.sh / build.ps1).
// This is for projects that already use Gradle and want to include the SDK
// as a source set or publish it locally (`gradle publishToMavenLocal` after
// adding `maven-publish`).
plugins {
    `java-library`
}

group = "io.github.bropines.glossa"
version = "1.0.0"

sourceSets {
    main { java.setSrcDirs(listOf("src")) }
    test { java.setSrcDirs(listOf("test")) }
}

tasks.withType<JavaCompile>().configureEach {
    options.release.set(17)
    options.encoding = "UTF-8"
}

// The tests are a plain main() with no framework.
val sdkTest by tasks.registering(JavaExec::class) {
    classpath = sourceSets["test"].runtimeClasspath
    mainClass.set("io.github.bropines.glossa.SdkTest")
}
tasks.named("check") { dependsOn(sdkTest) }
