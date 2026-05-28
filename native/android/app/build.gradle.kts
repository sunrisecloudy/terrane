plugins {
    id("com.android.application")
    id("org.jetbrains.kotlin.android")
}

val repoRoot = rootProject.projectDir.parentFile.parentFile
val generatedNativeAiAssets = layout.buildDirectory.dir("generated/native-ai-assets")
val syncNativeAiAssets by tasks.registering(Sync::class) {
    into(generatedNativeAiAssets)
    from(repoRoot.resolve("runtime-web")) {
        into("runtime")
    }
    from(repoRoot.resolve("webapps")) {
        into("webapps")
    }
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

    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_17
        targetCompatibility = JavaVersion.VERSION_17
    }

    sourceSets {
        getByName("main") {
            assets.srcDir(generatedNativeAiAssets)
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
}

dependencies {
    implementation("androidx.webkit:webkit:1.12.1")
}
