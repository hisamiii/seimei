import groovy.json.JsonSlurper
import java.util.Properties

plugins {
    id("com.android.application")
    id("org.jetbrains.kotlin.plugin.compose")
}

// Release signing lives outside the repo; android/publish.sh creates it once.
val signing = Properties().apply {
    val f = file(System.getProperty("user.home") + "/.local/share/seimei/signing/signing.properties")
    if (f.exists()) f.inputStream().use { load(it) }
}

// Firebase (for notifications) is configured from the console's google-services.json, kept outside the repo.
// Without it the app builds and works, only without notifications.
val firebase = file(System.getProperty("user.home") + "/.local/share/seimei/firebase/google-services.json")

@Suppress("UNCHECKED_CAST")
fun firebaseFields(pkg: String): Map<String, String> {
    if (!firebase.exists()) return emptyMap()
    val json = JsonSlurper().parse(firebase) as Map<String, Any?>
    val project = json["project_info"] as Map<String, Any?>
    val client = (json["client"] as List<Map<String, Any?>>).firstOrNull {
        ((it["client_info"] as Map<String, Any?>)["android_client_info"] as Map<String, Any?>)["package_name"] == pkg
    } ?: return emptyMap()
    return mapOf(
        "FCM_APP" to (client["client_info"] as Map<String, Any?>)["mobilesdk_app_id"] as String,
        "FCM_KEY" to ((client["api_key"] as List<Map<String, Any?>>).first()["current_key"] as String),
        "FCM_PROJECT" to project["project_id"] as String,
        "FCM_SENDER" to project["project_number"] as String,
    )
}

fun com.android.build.api.dsl.VariantDimension.firebase(pkg: String) {
    val f = firebaseFields(pkg)
    for (k in listOf("FCM_APP", "FCM_KEY", "FCM_PROJECT", "FCM_SENDER")) buildConfigField("String", k, "\"${f[k] ?: ""}\"")
}

android {
    namespace = "com.hisamiowo.seimei"
    // The theme token files the web page uses (#81), read as assets.
    sourceSets["main"].assets.srcDir("../../web/themes")
    compileSdk {
        version = release(37) { minorApiLevel = 2 }
    }

    defaultConfig {
        applicationId = "com.hisamiowo.seimei"
        minSdk = 30
        targetSdk = 36
        versionCode = (project.findProperty("seimei.versionCode") as String?)?.toInt() ?: 1
        versionName = (project.findProperty("seimei.versionName") as String?) ?: "0.0.0"
        // The server the first run starts with: -Pseimei.server=https://... (publish.sh passes config.toml's origin).
        // Without it the app asks for one.
        buildConfigField("String", "SERVER", "\"${project.findProperty("seimei.server") ?: ""}\"")
        // Debug builds may use a key without the fingerprint when the phone has none (emulators); release never does.
        buildConfigField("boolean", "TEST_KEY", "false")
        // The app updates itself from the paired server.
        // Store builds pass -Pseimei.updater=false: F-Droid and Play update the app themselves.
        buildConfigField("boolean", "UPDATER", "${project.findProperty("seimei.updater") ?: "true"}")
        firebase("com.hisamiowo.seimei")
    }

    // How notifications reach the phone: fcm through Firebase (needs a google-services.json), foss through UnifiedPush or a background connection.
    // Same app id, so one replaces the other.
    flavorDimensions += "push"
    productFlavors {
        create("fcm") { dimension = "push" }
        create("foss") { dimension = "push" }
    }

    signingConfigs {
        if (signing.isNotEmpty()) {
            create("release") {
                storeFile = file(signing.getProperty("storeFile"))
                storePassword = signing.getProperty("password")
                keyAlias = signing.getProperty("alias")
                keyPassword = signing.getProperty("password")
            }
        }
    }

    buildTypes {
        debug {
            applicationIdSuffix = ".debug"
            // The dev server through adb reverse; -Pseimei.server= (empty) starts at the server step instead.
            buildConfigField("String", "SERVER", "\"${project.findProperty("seimei.server") ?: "http://localhost:8418"}\"")
            buildConfigField("boolean", "TEST_KEY", "true")
            // The server's APK is a release build: another app id, so it would not install over this one.
            buildConfigField("boolean", "UPDATER", "false")
            firebase("com.hisamiowo.seimei.debug")
        }
        release {
            isMinifyEnabled = true
            isShrinkResources = true
            proguardFiles(getDefaultProguardFile("proguard-android-optimize.txt"), "proguard-rules.pro")
            if (signing.isNotEmpty()) signingConfig = signingConfigs.getByName("release")
        }
    }

    buildFeatures {
        compose = true
        buildConfig = true
    }

    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_21
        targetCompatibility = JavaVersion.VERSION_21
    }

    testOptions {
        unitTests.isReturnDefaultValues = true
    }
}

dependencies {
    implementation(platform("androidx.compose:compose-bom:2026.09.00"))
    implementation("androidx.compose.material3:material3")
    implementation("androidx.compose.ui:ui")
    implementation("androidx.activity:activity-compose:1.13.0")
    implementation("androidx.lifecycle:lifecycle-runtime-compose:2.11.0")
    implementation("androidx.lifecycle:lifecycle-viewmodel-compose:2.11.0")
    implementation("com.squareup.okhttp3:okhttp:5.5.0")
    "fcmImplementation"("com.google.firebase:firebase-messaging:25.1.3")
    "fossImplementation"("org.unifiedpush.android:connector:3.3.5")
    testImplementation("junit:junit:4.13.2")
    testImplementation("org.json:json:20250517")
}
