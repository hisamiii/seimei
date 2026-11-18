// What the phone app and the watch app share: the server's API, the chat model, Markdown, the theme tokens, shapes, icons and dictation.
// The theme token files themselves are each app's assets (web/themes).
plugins {
    id("com.android.library")
    id("org.jetbrains.kotlin.plugin.compose")
}

android {
    namespace = "com.hisamiowo.seimei.shared"
    compileSdk {
        version = release(37) { minorApiLevel = 2 }
    }

    defaultConfig {
        minSdk = 30
        // Phones, watches with a 32-bit system, and the emulator.
        ndk { abiFilters += listOf("arm64-v8a", "armeabi-v7a", "x86_64") }
    }

    // The echo canceller for voice calls: webrtc's audio processing, built from source by src/main/cpp/webrtc.sh.
    ndkVersion = "29.0.14206865"
    externalNativeBuild {
        cmake {
            path = file("src/main/cpp/CMakeLists.txt")
            version = "3.31.6"
        }
    }

    buildFeatures {
        compose = true
    }

    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_21
        targetCompatibility = JavaVersion.VERSION_21
    }
}

dependencies {
    api(platform("androidx.compose:compose-bom:2026.09.00"))
    api("androidx.compose.ui:ui")
    api("com.squareup.okhttp3:okhttp:5.5.0")
}
