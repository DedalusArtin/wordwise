import java.util.Properties
import org.jetbrains.kotlin.gradle.dsl.JvmTarget

plugins {
    id("com.android.application")
    id("org.jetbrains.kotlin.android")
    id("rust")
}

val tauriProperties = Properties().apply {
    val propFile = file("tauri.properties")
    if (propFile.exists()) {
        propFile.inputStream().use { load(it) }
    }
}

// ---- Android 正式发布签名 ----
//
// 为什么需要它：Tauri 生成的工程**不带任何 signingConfig**（上游模板就是如此），
// 于是 `assembleRelease` 只会产出 `app-arm64-release-unsigned.apk`。
// 未签名的 APK 在 Android 上是**装不上去**的（不是"会有警告"，是直接拒绝），
// 所以签名不是可选项。
//
// 密钥与口令放在仓库的 installer/android/ 下，并且被 .gitignore 排除 ——
// 理由见 .gitignore 里那段说明（签名决定了能否覆盖安装，不能进公开 git）。
//
// ★ 为什么用 rootProject.file("../..") 而不是把 storeFile 写进 properties：
//   properties 里写绝对路径会在换机器/换盘符后失效，而写相对路径又会相对
//   到 Gradle 的各个子项目目录（app/、tauri-android/……各不相同）。
//   固定成「相对 Gradle 根目录」这一种口径，两边都绕开了。
//
// ★ 缺失时**不报错、只降级**：全新 clone 或 CI 上没有密钥也应能跑通构建，
//   只是产物没签名。硬编码 require() 会让"能编译"变成"必须拿到私钥"。
val releaseKeystoreFile = rootProject.file("../../../installer/android/wordwise-release.jks")
val keystorePropertiesFile = rootProject.file("../../../installer/android/keystore.properties")
val keystoreProperties = Properties()
val hasReleaseKeystore = releaseKeystoreFile.exists() && keystorePropertiesFile.exists()
if (hasReleaseKeystore) {
    keystorePropertiesFile.inputStream().use { keystoreProperties.load(it) }
}

android {
    compileSdk = 37
    namespace = "com.dedalusartin.wordwise"
    defaultConfig {
        manifestPlaceholders["usesCleartextTraffic"] = "false"
        applicationId = "com.dedalusartin.wordwise"
        minSdk = 24
        targetSdk = 37
        versionCode = tauriProperties.getProperty("tauri.android.versionCode", "1").toInt()
        versionName = tauriProperties.getProperty("tauri.android.versionName", "1.0")
    }
    signingConfigs {
        if (hasReleaseKeystore) {
            create("release") {
                storeFile = releaseKeystoreFile
                storePassword = keystoreProperties.getProperty("storePassword")
                keyAlias = keystoreProperties.getProperty("keyAlias")
                keyPassword = keystoreProperties.getProperty("keyPassword")
                // v1 已在 Android 7.0 之后无用，但 minSdk = 24 仍建议保留以覆盖
                // "先校验 v1 再看 v2" 的老安装器；v2/v3 是 24+ 的必备项。
                enableV1Signing = true
                enableV2Signing = true
                enableV3Signing = true
            }
        }
    }
    buildTypes {
        getByName("debug") {
            manifestPlaceholders["usesCleartextTraffic"] = "true"
            isDebuggable = true
            isJniDebuggable = true
            isMinifyEnabled = false
            packaging {
                jniLibs.keepDebugSymbols.add("*/arm64-v8a/*.so")
                jniLibs.keepDebugSymbols.add("*/armeabi-v7a/*.so")
                jniLibs.keepDebugSymbols.add("*/x86/*.so")
                jniLibs.keepDebugSymbols.add("*/x86_64/*.so")
            }
        }
        getByName("release") {
            if (hasReleaseKeystore) {
                signingConfig = signingConfigs.getByName("release")
            }
            optimization {
               enable = true
            }
            proguardFiles(
                *fileTree(".") {
                  include("**/*.pro")
                  exclude("build/**")
                }.files.toTypedArray()
            )
        }
    }
    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_1_8
        targetCompatibility = JavaVersion.VERSION_1_8
    }
    buildFeatures {
        buildConfig = true
    }
}

kotlin {
    compilerOptions {
        jvmTarget = JvmTarget.JVM_1_8
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

apply(from = file("tauri.build.gradle.kts"))
