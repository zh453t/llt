plugins {
    id("com.android.application")
}

// 自用发布签名：`build.ps1 -Release` 首次运行时用 keytool 生成。
// 文件不存在时不配置 release 签名（这时只能打 debug 包）。
val keystore: File = rootProject.file("keystore/llt-release.jks")

android {
    namespace = "org.llt.timetable"
    compileSdk = 34
    // NDK 不在这里编：Rust 侧由 build.ps1 用 cargo ndk 编好放进 jniLibs。

    defaultConfig {
        applicationId = "org.llt.timetable"
        minSdk = 26
        targetSdk = 34
        // 旧的 cargo-apk 包用的是 16777472 这类大数，这里按日期递增，
        // 保证能覆盖安装（`INSTALL_FAILED_VERSION_DOWNGRADE` 的教训）。
        versionCode = 2026110801
        versionName = "0.1.0"
        // 只打这两个 ABI：真机（arm64）与模拟器（x86_64）。
        ndk {
            abiFilters += listOf("arm64-v8a", "x86_64")
        }
    }

    signingConfigs {
        if (keystore.exists()) {
            create("release") {
                storeFile = keystore
                storePassword = "llt-release"
                keyAlias = "llt"
                keyPassword = "llt-release"
            }
        }
    }

    buildTypes {
        debug {
            applicationIdSuffix = ".debug"
            versionNameSuffix = "-debug"
        }
        release {
            isMinifyEnabled = false
            isShrinkResources = false
            if (keystore.exists()) {
                signingConfig = signingConfigs.getByName("release")
            }
        }
    }

    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_17
        targetCompatibility = JavaVersion.VERSION_17
    }

    packaging {
        // 压缩 .so：两个 ABI 的 .so 加起来 30 MB，不压的话 APK 直接 29 MB。
        // 代价是安装时把它们解到 /data（多占一份空间），启动不用解压。
        jniLibs.useLegacyPackaging = true
    }

    lint {
        // 没有 androidx，MinSdk 检查意义不大；别的警告照常。
        disable += setOf("ExpiredTargetSdkVersion", "OldTargetApi")
        abortOnError = false
    }
}

dependencies {
    // 纯 NativeActivity + 手写 JNI，不引入 androidx。
}
