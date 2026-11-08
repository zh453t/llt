// AGP 用 buildscript classpath 引入，而不是 `plugins {}` 块。
// 原因：插件标记（marker POM）在部分镜像上解析不稳定，而 `com.android.tools.build:gradle`
// 本体在各镜像上都有；两者效果相同，后者更皮实。
buildscript {
    repositories {
        // 国内镜像优先：AGP 在 dl.google.com 上，直连常年几 KB/s。
        maven("https://maven.aliyun.com/repository/google")
        maven("https://maven.aliyun.com/repository/public")
        google()
        mavenCentral()
    }
    dependencies {
        classpath("com.android.tools.build:gradle:9.5.1")
    }
}
