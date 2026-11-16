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
