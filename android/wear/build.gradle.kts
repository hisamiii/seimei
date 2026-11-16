import groovy.json.JsonSlurper
import java.util.Properties

plugins {
    id("com.android.application")
    id("org.jetbrains.kotlin.plugin.compose")
}

// The same release key and Firebase project as the phone app (see app/build.gradle.kts): the watch app has the phone app's id, as Wear OS expects.
val signing = Properties().apply {
    val f = file(System.getProperty("user.home") + "/.local/share/seimei/signing/signing.properties")
    if (f.exists()) f.inputStream().use { load(it) }
}
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
    // The phone's notifications stay mirrored to the watch unless the watch gets its own.
    manifestPlaceholders["bridge"] = if (f.isEmpty()) "BRIDGING" else "NO_BRIDGING"
}

android {
    namespace = "com.hisamiowo.seimei.wear"
    sourceSets.getByName("main").assets.srcDir("../../web/themes")
    compileSdk {
        version = release(37) { minorApiLevel = 2 }
    }

    defaultConfig {
        applicationId = "com.hisamiowo.seimei"
        minSdk = 33
        targetSdk = 36
        versionCode = (project.findProperty("seimei.versionCode") as String?)?.toInt() ?: 1
        versionName = (project.findProperty("seimei.versionName") as String?) ?: "0.0.0"
        // The server the first run pairs with: -Pseimei.server=https://...; without it the watch asks for one.
        buildConfigField("String", "SERVER", "\"${project.findProperty("seimei.server") ?: ""}\"")
        firebase("com.hisamiowo.seimei")
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
}

dependencies {
    implementation(project(":shared"))
    implementation(platform("androidx.compose:compose-bom:2026.09.00"))
    implementation("androidx.compose.ui:ui")
    implementation("androidx.wear.compose:compose-material3:1.7.1")
    implementation("androidx.wear.compose:compose-foundation:1.7.1")
    implementation("androidx.activity:activity-compose:1.13.0")
    implementation("androidx.lifecycle:lifecycle-runtime-compose:2.11.0")
    implementation("androidx.lifecycle:lifecycle-viewmodel-compose:2.11.0")
    implementation("com.google.firebase:firebase-messaging:25.1.3")
}
