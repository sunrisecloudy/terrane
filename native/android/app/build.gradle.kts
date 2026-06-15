plugins {
    id("com.android.application")
    id("org.jetbrains.kotlin.android")
}

val repoRoot = rootProject.projectDir.parentFile.parentFile
val generatedNativeAiAssets = layout.buildDirectory.dir("generated/terrane-assets")
val generatedForgeFfiJniLibs = layout.buildDirectory.dir("generated/terrane-forge-ffi/jniLibs")
val androidMinSdk = 26

data class AndroidForgeFfiTarget(
    val abi: String,
    val rustTarget: String,
    val clangPrefix: String,
)

fun existingEnvDir(vararg names: String): File? =
    names.asSequence()
        .mapNotNull { System.getenv(it)?.takeIf(String::isNotBlank)?.let(::File) }
        .firstOrNull { it.isDirectory }

fun androidSdkDir(): File? =
    existingEnvDir("ANDROID_HOME", "ANDROID_SDK_ROOT")
        ?: File(System.getProperty("user.home"), "Library/Android/sdk").takeIf { it.isDirectory }

fun androidNdkDir(): File? =
    existingEnvDir("ANDROID_NDK_HOME", "ANDROID_NDK_ROOT")
        ?: androidSdkDir()
            ?.resolve("ndk")
            ?.listFiles()
            ?.filter { it.isDirectory }
            ?.maxByOrNull { it.name }

fun ndkPrebuiltBinDir(ndkDir: File): File? {
    val os = System.getProperty("os.name").lowercase()
    val arch = System.getProperty("os.arch").lowercase()
    val hostTags = when {
        os.contains("mac") && arch.contains("aarch64") -> listOf("darwin-arm64", "darwin-x86_64")
        os.contains("mac") -> listOf("darwin-x86_64", "darwin-arm64")
        os.contains("linux") -> listOf("linux-x86_64")
        os.contains("windows") -> listOf("windows-x86_64")
        else -> emptyList()
    }
    return hostTags
        .map { ndkDir.resolve("toolchains/llvm/prebuilt/$it/bin") }
        .firstOrNull { it.isDirectory }
}

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
val androidForgeFfiTargets = listOf(
    AndroidForgeFfiTarget("arm64-v8a", "aarch64-linux-android", "aarch64-linux-android"),
    AndroidForgeFfiTarget("armeabi-v7a", "armv7-linux-androideabi", "armv7a-linux-androideabi"),
    AndroidForgeFfiTarget("x86", "i686-linux-android", "i686-linux-android"),
    AndroidForgeFfiTarget("x86_64", "x86_64-linux-android", "x86_64-linux-android"),
)
val buildAndroidForgeFfiAbiTasks = androidForgeFfiTargets.map { targetSpec ->
    val abi = targetSpec.abi
    val target = targetSpec.rustTarget
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
            val ndkDir = androidNdkDir()
                ?: throw GradleException("Android NDK is required to build Forge FFI for $abi")
            val ndkBinDir = ndkPrebuiltBinDir(ndkDir)
                ?: throw GradleException("Android NDK LLVM prebuilt toolchain was not found under ${ndkDir.absolutePath}")
            val clang = ndkBinDir.resolve("${targetSpec.clangPrefix}$androidMinSdk-clang")
            val ar = ndkBinDir.resolve("llvm-ar")
            if (!clang.isFile) {
                throw GradleException("Android NDK clang was not found: ${clang.absolutePath}")
            }
            if (!ar.isFile) {
                throw GradleException("Android NDK llvm-ar was not found: ${ar.absolutePath}")
            }
            val targetEnv = target.replace("-", "_")
            val cargoTargetEnv = targetEnv.uppercase()
            environment("CC_$targetEnv", clang.absolutePath)
            environment("AR_$targetEnv", ar.absolutePath)
            environment("CARGO_TARGET_${cargoTargetEnv}_LINKER", clang.absolutePath)
            environment("CARGO_TARGET_${cargoTargetEnv}_AR", ar.absolutePath)
            environment("PATH", "${ndkBinDir.absolutePath}${File.pathSeparator}${System.getenv("PATH").orEmpty()}")
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
        minSdk = androidMinSdk
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
