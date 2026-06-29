// snapdir Java binding — Gradle Kotlin DSL build file.
//
// NOTE: The gatesmith CI image does NOT have Gradle installed; the gate
// verification compiles via raw `javac`. This file is shipped for consumers
// who DO have Gradle, and documents the required compiler/runtime flags.
//
// Usage (with Gradle installed):
//   ./gradlew compileJava
//   ./gradlew jar

plugins {
    java
    `maven-publish`
    signing
}

group   = "org.snapdir"
version = "1.11.0"

java {
    sourceCompatibility = JavaVersion.VERSION_17
    targetCompatibility = JavaVersion.VERSION_17
    withSourcesJar()
    withJavadocJar()
}

repositories {
    mavenCentral()
}

// JDK 17 incubator foreign API flags.
// These are required at both compile time and runtime.
val foreignFlags = listOf(
    "--add-modules", "jdk.incubator.foreign"
)
val runtimeForeignFlags = listOf(
    "--add-modules", "jdk.incubator.foreign",
    "--enable-native-access=ALL-UNNAMED"
)

tasks.withType<JavaCompile>().configureEach {
    options.compilerArgs.addAll(foreignFlags)
    options.encoding = "UTF-8"
}

tasks.withType<Test>().configureEach {
    jvmArgs(runtimeForeignFlags)
}

tasks.withType<JavaExec>().configureEach {
    jvmArgs(runtimeForeignFlags)
}

// Include native libraries from src/main/resources in the JAR.
// The gitignored .so/.dylib/.dll files must be placed here before packaging.
sourceSets {
    main {
        resources {
            srcDir("src/main/resources")
        }
    }
}

dependencies {
    testImplementation("org.junit.jupiter:junit-jupiter:5.10.0")
    testRuntimeOnly("org.junit.platform:junit-platform-launcher")
}

tasks.test {
    useJUnitPlatform()
}

// JPMS automatic module name — matches the manifest written by the de-gradled
// gate verification so a credited-CI Gradle build produces the same artifact.
tasks.jar {
    manifest {
        attributes(
            "Automatic-Module-Name" to "io.snapdir",
            "Implementation-Title" to "snapdir"
        )
    }
}

publishing {
    publications {
        create<MavenPublication>("mavenJava") {
            from(components["java"])
            pom {
                name.set("snapdir")
                description.set("Java binding for the snapdir-ffi C ABI via JDK 17 Foreign Function API")
                url.set("https://snapdir.org")
                licenses {
                    license {
                        name.set("MIT OR Apache-2.0")
                        url.set("https://github.com/snapdir/snapdir/blob/main/LICENSE")
                        distribution.set("repo")
                    }
                }
                scm {
                    connection.set("scm:git:https://github.com/snapdir/snapdir.git")
                    developerConnection.set("scm:git:https://github.com/snapdir/snapdir.git")
                    url.set("https://github.com/snapdir/snapdir")
                }
                developers {
                    developer {
                        id.set("snapdir")
                        name.set("snapdir maintainers")
                        email.set("maintainers@snapdir.org")
                        url.set("https://snapdir.org")
                        organization.set("snapdir")
                        organizationUrl.set("https://snapdir.org")
                    }
                }
            }
        }
    }
    repositories {
        maven {
            name = "central"
            url = uri("https://central.sonatype.com/api/v1/publisher/upload")
            credentials {
                username = System.getenv("CENTRAL_USERNAME") ?: ""
                password = System.getenv("CENTRAL_PASSWORD") ?: ""
            }
        }
    }
}

signing {
    val signingKey: String? = System.getenv("GPG_SIGNING_KEY")
    val signingPassword: String? = System.getenv("GPG_SIGNING_PASSWORD")
    if (signingKey != null && signingPassword != null) {
        useInMemoryPgpKeys(signingKey, signingPassword)
    }
    sign(publishing.publications["mavenJava"])
}
