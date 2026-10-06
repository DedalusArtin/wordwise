// buildSrc 是 Gradle 隐式创建的**独立构建**，读不到根目录 settings.gradle 里的
// pluginManagement 配置，所以 `kotlin-dsl`（org.gradle.kotlin.kotlin-dsl）这条
// 插件解析必须在这里再给一份可达的仓库。
//
// 背景与实测见根目录 settings.gradle：plugins.gradle.org 在国内直连不通，
// 走本地 HTTP 代理也超时。
pluginManagement {
    repositories {
        maven("https://maven.aliyun.com/repository/gradle-plugin")
        gradlePluginPortal()
    }
}
