plugins {
    id("com.android.application")
    id("org.jetbrains.kotlin.android")
}

val repoRoot = rootProject.projectDir.parentFile.parentFile
val generatedNativeAiAssets = layout.buildDirectory.dir("generated/terrane-assets")
val generatedForgeFfiJniLibs = layout.buildDirectory.dir("generated/terrane-forge-ffi/jniLibs")
val syncNativeAiAssets by tasks.registering(Sync::class) {
    into(generatedNativeAiAssets)
    from(repoRoot.resolve("runtime-web")) {
        into("runtime")
    }
    from(repoRoot.resolve("webapps")) {
        into("webapps")
    }
    from(repoRoot.resolve("db/sqlite")) {
        into("db/sqlite")
    }
}
val androidForgeFfiTargets = mapOf(
    "arm64-v8a" to "aarch64-linux-android",
    "armeabi-v7a" to "armv7-linux-androideabi",
    "x86" to "i686-linux-android",
    "x86_64" to "x86_64-linux-android",
)
val buildAndroidForgeFfiAbiTasks = androidForgeFfiTargets.map { (abi, target) ->
    val taskName = "buildAndroidForgeFfi${abi.replace("-", "").replace("_", "")}"
    tasks.register<Exec>(taskName) {
        val forgeDir = repoRoot.resolve("forge")
        val cargoTargetDir = layout.buildDirectory.dir("cargo/android/$abi").get().asFile
        val outputDir = generatedForgeFfiJniLibs.get().asFile.resolve(abi)
        val outputFile = outputDir.resolve("libforge_ffi.so")
        val builtLibrary = cargoTargetDir.resolve("$target/debug/libforge_ffi.so")
        inputs.dir(forgeDir.resolve("crates"))
        inputs.file(forgeDir.resolve("Cargo.toml"))
        inputs.file(forgeDir.resolve("Cargo.lock"))
        inputs.property("cargoTarget", target)
        outputs.file(outputFile)
        workingDir = forgeDir
        environment("CARGO_TARGET_DIR", cargoTargetDir.absolutePath)
        doFirst {
            outputDir.mkdirs()
        }
        commandLine(
            "cargo",
            "build",
            "-p",
            "forge-ffi",
            "--locked",
            "--target",
            target,
        )
        doLast {
            copy {
                from(builtLibrary)
                into(outputDir)
            }
        }
    }
}
val buildAndroidForgeFfi by tasks.registering {
    dependsOn(buildAndroidForgeFfiAbiTasks)
}

android {
    namespace = "com.terrane.platform"
    compileSdk = 35

    defaultConfig {
        applicationId = "com.terrane.platform"
        minSdk = 26
        targetSdk = 35
        versionCode = 1
        versionName = "0.1.0"

        externalNativeBuild {
            cmake {
                cppFlags += "-std=c++17"
            }
        }
    }

    buildFeatures {
        buildConfig = true
    }

    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_17
        targetCompatibility = JavaVersion.VERSION_17
    }

    sourceSets {
        getByName("main") {
            assets.srcDir(generatedNativeAiAssets)
            jniLibs.srcDir(generatedForgeFfiJniLibs)
        }
    }

    externalNativeBuild {
        cmake {
            path = file("src/main/cpp/CMakeLists.txt")
        }
    }
}

kotlin {
    jvmToolchain(17)
}

tasks.named("preBuild") {
    dependsOn(syncNativeAiAssets)
    dependsOn(buildAndroidForgeFfi)
}

dependencies {
    implementation("androidx.activity:activity-ktx:1.9.3")
    implementation("androidx.webkit:webkit:1.12.1")
    implementation("com.squareup.okhttp3:okhttp:4.12.0")
}
