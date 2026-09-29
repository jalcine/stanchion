import java.io.ByteArrayOutputStream

plugins {
    id("com.android.application")
    id("org.jetbrains.kotlin.android")
    id("org.jetbrains.kotlin.plugin.compose")
}

val uniffiDir = layout.buildDirectory.dir("generated/uniffi")
val jniDir = layout.buildDirectory.dir("jniLibs")

android {
    namespace = "com.example.stanchiondemo"
    compileSdk = 34
    // Keep in sync with ANDROID_NDK_VERSION / ANDROID_API in the root mise.toml.
    ndkVersion = "27.3.13750724"

    defaultConfig {
        applicationId = "com.example.stanchiondemo"
        minSdk = 26
        targetSdk = 34
        versionCode = 1
        versionName = "1.0"
        ndk {
            // Minimal set: arm64-v8a (devices) + x86_64 (emulator).
            abiFilters += listOf("arm64-v8a", "x86_64")
        }
    }

    buildTypes {
        getByName("debug") {
            isDebuggable = true
        }
        getByName("release") {
            isMinifyEnabled = false
        }
    }

    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_17
        targetCompatibility = JavaVersion.VERSION_17
    }

    kotlin {
        compilerOptions {
            jvmTarget.set(org.jetbrains.kotlin.gradle.dsl.JvmTarget.JVM_17)
        }
    }

    buildFeatures {
        compose = true
    }

    sourceSets {
        getByName("main") {
            // UniFFI Kotlin and the Rust .so files are generated at build time
            // (mise run android:build:ffi, or generateStanchionBindings below)
            // and never committed.
            kotlin.srcDirs("src/main/java", uniffiDir)
            jniLibs.srcDirs(jniDir)
            assets.srcDirs("src/main/assets")
        }
    }
}

// Regenerates the UniFFI Kotlin + device .so files at build time so the
// checked-in tree carries no generated artifacts.
tasks.register("generateStanchionBindings") {
    group = "stanchion"
    description = "Build host lib, generate UniFFI Kotlin, build device .so files."
    val repo = rootDir.parentFile.parentFile
    // Re-run only when Rust sources change; otherwise reuse app/build outputs.
    inputs.dir(repo.resolve("crates"))
    inputs.dir(repo.resolve("bindings/uniffi/src"))
    outputs.dir(uniffiDir)
    outputs.dir(jniDir)
    doLast {
        // cargo-ndk honors ANDROID_NDK_HOME over the SDK default, and a stale
        // shell export would point it at a nonexistent tree. Pin it here.
        val ndkHome = android.sdkDirectory.resolve("ndk").resolve("27.3.13750724").absolutePath
        exec {
            workingDir = repo
            commandLine("rustup", "target", "add", "aarch64-linux-android", "x86_64-linux-android")
        }
        exec {
            workingDir = repo
            commandLine("cargo", "build", "-p", "stanchion-uniffi")
        }
        val lib = repo.resolve("target/debug/libstanchion_uniffi.so").absolutePath
        exec {
            workingDir = repo
            commandLine(
                "cargo", "run", "-q", "-p", "stanchion-uniffi", "--bin", "uniffi-bindgen",
                "--", "generate", "--library", lib, "--language", "kotlin",
                "--out-dir", uniffiDir.get().asFile.absolutePath,
            )
        }
        exec {
            workingDir = repo
            environment("ANDROID_NDK_HOME", ndkHome)
            commandLine(
                "cargo", "ndk", "-t", "arm64-v8a", "-t", "x86_64",
                "-o", jniDir.get().asFile.absolutePath,
                "build", "-p", "stanchion-uniffi", "--release",
            )
        }
    }
}

tasks.named("preBuild") {
    dependsOn("generateStanchionBindings")
}

dependencies {
    // Same JNA + coroutines the JVM smoke test pins in bindings/uniffi/smoke-kotlin.sh.
    implementation("net.java.dev.jna:jna:5.14.0")
    implementation("org.jetbrains.kotlinx:kotlinx-coroutines-android:1.8.1")

    val composeBom = platform("androidx.compose:compose-bom:2024.06.00")
    implementation(composeBom)
    androidTestImplementation(composeBom)

    implementation("androidx.core:core-ktx:1.13.1")
    implementation("androidx.lifecycle:lifecycle-runtime-compose:2.7.0")
    implementation("androidx.activity:activity-compose:1.9.2")
    implementation("androidx.compose.ui:ui")
    implementation("androidx.compose.material3:material3")
    implementation("androidx.compose.ui:ui-tooling-preview")
    debugImplementation("androidx.compose.ui:ui-tooling")
}
