plugins {
    id("com.android.application")
    id("org.jetbrains.kotlin.android")
}

val repoRoot = rootProject.projectDir.parentFile.parentFile
val generatedNativeAiAssets = layout.buildDirectory.dir("generated/native-ai-assets")
val generatedZigCoreJniLibs = layout.buildDirectory.dir("generated/native-ai-zig-core/jniLibs")
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
val androidZigCoreTargets = mapOf(
    "arm64-v8a" to "aarch64-linux-android",
    "armeabi-v7a" to "arm-linux-androideabi",
    "x86" to "x86-linux-android",
    "x86_64" to "x86_64-linux-android",
)
val buildAndroidZigCoreAbiTasks = androidZigCoreTargets.map { (abi, target) ->
    val taskName = "buildAndroidZigCore${abi.replace("-", "").replace("_", "")}"
    tasks.register<Exec>(taskName) {
        val zigCoreDir = repoRoot.resolve("zig-core")
        val outputDir = generatedZigCoreJniLibs.get().asFile.resolve(abi)
        val outputFile = outputDir.resolve("libzig_core.so")
        inputs.dir(zigCoreDir.resolve("src"))
        inputs.property("zigTarget", target)
        inputs.property("zigSoname", "libzig_core.so")
        outputs.file(outputFile)
        workingDir = zigCoreDir
        environment("ZIG_GLOBAL_CACHE_DIR", layout.buildDirectory.dir("zig-cache/android/global").get().asFile.absolutePath)
        environment("ZIG_LOCAL_CACHE_DIR", layout.buildDirectory.dir("zig-cache/android/local-$abi").get().asFile.absolutePath)
        doFirst {
            outputDir.mkdirs()
        }
                commandLine(
                    "zig",
                    "build-lib",
                    "src/lib.zig",
                    "--name",
                    "zig_core",
                    "-dynamic",
                    "-target",
                    target,
                    "-fsoname=libzig_core.so",
                    "-femit-bin=${outputFile.absolutePath}",
                )
    }
}
val buildAndroidZigCore by tasks.registering {
    dependsOn(buildAndroidZigCoreAbiTasks)
}

android {
    namespace = "com.nativeai.platform"
    compileSdk = 35

    defaultConfig {
        applicationId = "com.nativeai.platform"
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
            jniLibs.srcDir(generatedZigCoreJniLibs)
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
    dependsOn(buildAndroidZigCore)
}

dependencies {
    implementation("androidx.activity:activity-ktx:1.9.3")
    implementation("androidx.webkit:webkit:1.12.1")
}
