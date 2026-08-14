import groovy.json.JsonOutput
import groovy.json.JsonSlurper
import java.util.Properties

plugins {
    id("com.android.application")
    id("org.jetbrains.kotlin.android")
    id("rust")
}

layout.buildDirectory.set(file(providers.gradleProperty("solumBuildDir").orElse("build").get()))

val rootTauriConfigFile = file("../../../tauri.conf.json")
require(rootTauriConfigFile.isFile) {
    "Missing root Tauri config: ${rootTauriConfigFile.absolutePath}"
}
@Suppress("UNCHECKED_CAST")
val rootTauriConfig = (JsonSlurper().parse(rootTauriConfigFile) as? Map<String, Any?>)
    ?: error("Root tauri.conf.json must contain a JSON object")
val tauriVersionName = (rootTauriConfig["version"] as? String)?.trim()
    ?.takeIf { it.matches(Regex("""\d+\.\d+\.\d+(?:-[0-9A-Za-z.-]+)?(?:\+[0-9A-Za-z.-]+)?""")) }
    ?: error("tauri.conf.json version must be a non-empty semantic version")
val tauriBundle = rootTauriConfig["bundle"] as? Map<*, *>
    ?: error("tauri.conf.json bundle must contain a JSON object")
val tauriAndroidBundle = tauriBundle["android"] as? Map<*, *>
    ?: error("tauri.conf.json bundle.android must contain a JSON object")
val tauriVersionCodeText = tauriAndroidBundle["versionCode"]?.toString()
    ?: error("tauri.conf.json bundle.android.versionCode is required")
val tauriVersionCode = tauriVersionCodeText.toIntOrNull()
    ?.takeIf { it > 0 && it.toString() == tauriVersionCodeText }
    ?: error("tauri.conf.json bundle.android.versionCode must be a positive 32-bit integer")

val generatedTauriProperties = file("tauri.properties")
val generatedTauriAsset = file("src/main/assets/tauri.conf.json")
val syncTauriGeneratedInputs by tasks.registering {
    group = "build setup"
    description = "Synchronize Android metadata and runtime config from the root Tauri config"
    inputs.file(rootTauriConfigFile)
    outputs.files(generatedTauriProperties, generatedTauriAsset)
    doLast {
        generatedTauriProperties.parentFile.mkdirs()
        generatedTauriProperties.writeText(
            "tauri.android.versionName=$tauriVersionName\n" +
                "tauri.android.versionCode=$tauriVersionCode\n",
            Charsets.UTF_8,
        )

        val runtimeConfig = LinkedHashMap(rootTauriConfig)
        val plugins = runtimeConfig["plugins"]
        if (plugins == null) {
            runtimeConfig["plugins"] = emptyMap<String, Any?>()
        } else if (plugins !is Map<*, *>) {
            error("tauri.conf.json plugins must contain a JSON object")
        }
        generatedTauriAsset.parentFile.mkdirs()
        generatedTauriAsset.writeText(
            JsonOutput.prettyPrint(JsonOutput.toJson(runtimeConfig)) + "\n",
            Charsets.UTF_8,
        )
    }
}

tasks.named("preBuild").configure {
    dependsOn(syncTauriGeneratedInputs)
}

// Release signing (self-use sideload, no Play Store): key material lives in
// `keystore.properties` + the keystore file it points at, both gitignored —
// same "local secret file, never committed" pattern as solum-llm.json /
// solum-sync.json. Absent on a fresh checkout, in which case `release` stays
// unsigned (buildable, just not installable) rather than failing the build.
val keystoreProperties = Properties().apply {
    val propFile = file("keystore.properties")
    if (propFile.exists()) {
        propFile.inputStream().use { load(it) }
    }
}

android {
    compileSdk = 36
    namespace = "dev.solum.app"
    defaultConfig {
        manifestPlaceholders["usesCleartextTraffic"] = "false"
        applicationId = "dev.solum.app"
        // Bumped from 24 → 26 (2026-07-12): solum-health-connect requires API 26
        // (Health Connect client library minimum), and Gradle's manifest
        // merger requires the app's minSdk to be >= every library's.
        minSdk = 26
        targetSdk = 36
        versionCode = tauriVersionCode
        versionName = tauriVersionName
    }
    signingConfigs {
        if (keystoreProperties.containsKey("storeFile")) {
            create("release") {
                storeFile = file(keystoreProperties.getProperty("storeFile"))
                storePassword = keystoreProperties.getProperty("storePassword")
                keyAlias = keystoreProperties.getProperty("keyAlias")
                keyPassword = keystoreProperties.getProperty("keyPassword")
            }
        }
    }
    buildTypes {
        getByName("debug") {
            manifestPlaceholders["usesCleartextTraffic"] = "true"
            isDebuggable = true
            isJniDebuggable = true
            isMinifyEnabled = false
            packaging {                jniLibs.keepDebugSymbols.add("*/arm64-v8a/*.so")
                jniLibs.keepDebugSymbols.add("*/armeabi-v7a/*.so")
                jniLibs.keepDebugSymbols.add("*/x86/*.so")
                jniLibs.keepDebugSymbols.add("*/x86_64/*.so")
            }
        }
        getByName("release") {
            isMinifyEnabled = true
            proguardFiles(
                *fileTree(".") { include("**/*.pro") }
                    .plus(getDefaultProguardFile("proguard-android-optimize.txt"))
                    .toList().toTypedArray()
            )
            if (keystoreProperties.containsKey("storeFile")) {
                signingConfig = signingConfigs.getByName("release")
            }
        }
    }
    kotlinOptions {
        jvmTarget = "1.8"
    }
    buildFeatures {
        buildConfig = true
    }
}

rust {
    rootDirRel = "../../../"
}

dependencies {
    implementation("androidx.webkit:webkit:1.14.0")
    implementation("androidx.appcompat:appcompat:1.7.1")
    implementation("androidx.activity:activity-ktx:1.10.1")
    implementation("com.google.android.material:material:1.12.0")
    implementation("androidx.lifecycle:lifecycle-process:2.10.0")
    testImplementation("junit:junit:4.13.2")
    androidTestImplementation("androidx.test.ext:junit:1.1.4")
    androidTestImplementation("androidx.test.espresso:espresso-core:3.5.0")
}

apply(from = "tauri.build.gradle.kts")
