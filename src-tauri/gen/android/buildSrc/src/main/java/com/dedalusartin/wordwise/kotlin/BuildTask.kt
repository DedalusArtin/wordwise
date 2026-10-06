import java.io.File
import org.apache.tools.ant.taskdefs.condition.Os
import org.gradle.api.DefaultTask
import org.gradle.api.GradleException
import org.gradle.api.logging.LogLevel
import org.gradle.api.tasks.Input
import org.gradle.api.tasks.TaskAction
import javax.inject.Inject
import org.gradle.process.ExecOperations

/// target 短名 → Rust 的 triple。
/// 值必须与 cargo-mobile2 里的映射一致（它是 cargo 的实际目录名，写错就找不到产物）。
private val TRIPLE_BY_TARGET = mapOf(
    "aarch64" to "aarch64-linux-android",
    "armv7" to "armv7-linux-androideabi",
    "i686" to "i686-linux-android",
    "x86_64" to "x86_64-linux-android",
)

/// target 短名 → APK 里的 ABI 目录名。
/// 注意 arm64-v8a 是 Android 的叫法，不是 Rust 的 —— 两边不能混用。
private val ABI_BY_TARGET = mapOf(
    "aarch64" to "arm64-v8a",
    "armv7" to "armeabi-v7a",
    "i686" to "x86",
    "x86_64" to "x86_64",
)

abstract class BuildTask : DefaultTask() {
    @get:Inject
    abstract val execOperations: ExecOperations

    @Input
    var rootDirRel: String? = null
    @Input
    var projectDir: String? = null
    @Input
    var target: String? = null
    @Input
    var release: Boolean? = null

    @TaskAction
    fun assemble() {
        val executable = """cargo""";
        var succeeded = false
        var firstError: Exception? = null

        try {
            runTauriCli(executable)
            succeeded = true
        } catch (e: Exception) {
            firstError = e
            if (Os.isFamily(Os.FAMILY_WINDOWS)) {
                // Try different Windows-specific extensions
                val fallbacks = listOf(
                    "$executable.exe",
                    "$executable.cmd",
                    "$executable.bat",
                )

                for (fallback in fallbacks) {
                    try {
                        runTauriCli(fallback)
                        succeeded = true
                        break
                    } catch (fallbackException: Exception) {
                        firstError.addSuppressed(fallbackException)
                    }
                }
            }
        }

        // ★ 抛出**第一个**异常，不是最后一个。
        //
        // 上游原本是 `throw lastException`，而候选列表的最后两个是 cargo.cmd /
        // cargo.bat —— 这两个在 Windows 上通常**根本不存在**，于是最终抛出的永远
        // 是 "Cannot run program \"cargo.bat\": CreateProcess error=2"。
        // 真正的原因是第一个 cargo 跑起来后**退出码非 0**，它连同 stderr 一起
        // 被覆盖掉了。本轮排查时 Gradle 只反复报 "A problem occurred starting
        // process 'command 'cargo.bat''"，而真实错误其实是 tauri-cli 报的
        // "does not include required runtime symbols"，为此多绕了很多弯路。
        // 现在主异常是第一个（cargo 本体的真实失败），其余作为 suppressed 保留。
        val failure = firstError
        if (!succeeded && failure != null) {
            throw failure
        }

        materializeJniLibs()
    }

    fun runTauriCli(executable: String) {
        val rootDirRel = rootDirRel ?: throw GradleException("rootDirRel cannot be null")
        val target = target ?: throw GradleException("target cannot be null")
        val release = release ?: throw GradleException("release cannot be null")
        val args = listOf("tauri", "android", "android-studio-script");

        execOperations.exec {
            workingDir(File(projectDir, rootDirRel))
            executable(executable)
            args(args)
            if (logger.isEnabled(LogLevel.DEBUG)) {
                args("-vv")
            } else if (logger.isEnabled(LogLevel.INFO)) {
                args("-v")
            }
            if (release) {
                args("--release")
            }
            args(listOf("--target", target))
        }.assertNormalExitValue()
    }

    /// 把 cargo 的真实产物落到 `app/src/main/jniLibs/<abi>/` 下。
    ///
    /// **为什么必须补这一步**：tauri-cli 走的是 cargo-mobile2，后者不拷贝、而是
    /// 在 jniLibs 下建一个**指向 cargo 产物的符号链接**（`os::windows::ln::force_symlink`）。
    /// 而 Windows 上创建符号链接需要「开发者模式」或管理员权限，这条路并不总是通：
    ///
    ///   - 拿不到权限时，`symlink_file` 返回 ERROR_PRIVILEGE_NOT_HELD，构建直接失败。
    ///     上游的报错文案（"You should use developer mode"）还算清楚，属于可接受；
    ///   - **真正危险的是第二种**：某些受限环境（本轮实测的构建沙箱就是）会把符号链接
    ///     创建**静默降级**成一个 **0 字节的普通文件**，`ln` 返回成功、日志里也打印了
    ///     "symlinking lib ..."，构建一路绿灯 —— 直到把 APK 装进手机，启动瞬间闪退。
    ///     实测这一版 APK 就是 1.5 MB（正常应有十几 MB），包内
    ///     `lib/arm64-v8a/libwordwise_lib.so` 是 0 字节。
    ///
    /// 这里改成**确定性拷贝**：不依赖任何链接权限，行为在哪个环境下都一样。
    /// 代价是每次多拷十几 MB，相对「构建成功但产物是废的」完全可以接受。
    ///
    /// 找不到真实产物时只告警、不抛异常：多 ABI 构建时某个 target 没编出来，
    /// 应该由它自己的任务报错，而不是在这里变成一条看不懂的失败。
    private fun materializeJniLibs() {
        val target = target ?: return
        val release = release ?: return
        val rootDirRel = rootDirRel ?: return

        val abi = ABI_BY_TARGET[target] ?: return
        val triple = TRIPLE_BY_TARGET[target] ?: return
        val profile = if (release) "release" else "debug"

        // rootDirRel 与 runTauriCli 用的是同一个基准（都相对 app/），
        // 所以这里算出来的就是 src-tauri/。
        val root = File(projectDir, rootDirRel)
        val profileDir = cargoProfileDir(root, triple, profile) ?: run {
            logger.warn("[rust] 找不到 $triple/$profile 的产物目录，跳过 jniLibs 落地")
            return
        }
        val jniDir = File(root, "gen/android/app/src/main/jniLibs/$abi")
        if (!jniDir.isDirectory) {
            logger.warn("[rust] jniLibs 目录不存在：${jniDir.absolutePath}")
            return
        }

        val libs = jniDir.listFiles { f -> f.isFile && f.name.startsWith("lib") && f.name.endsWith(".so") }
            ?: emptyArray()
        for (lib in libs) {
            val src = File(profileDir, lib.name)
            if (!src.isFile || src.length() == 0L) {
                logger.warn("[rust] ${lib.name} 的真实产物不在 ${src.absolutePath}，jniLibs 里的文件保持原样")
                continue
            }
            // 先删再拷：符号链接、0 字节占位文件、真文件三种形态都能覆盖。
            // 直接复制到符号链接上会写到链接的**目标**去，那是 cargo 的产物目录，不能动。
            if (!lib.delete()) {
                logger.warn("[rust] 无法删除 ${lib.absolutePath}，跳过替换")
                continue
            }
            src.copyTo(lib, overwrite = true)
            logger.lifecycle("[rust] jniLibs/$abi/${lib.name} <- ${src.absolutePath}（${lib.length()} 字节）")
        }
    }

    /// cargo 的产物目录，按 cargo 自己的优先级找：CARGO_TARGET_DIR 优先，否则 `<包目录>/target`。
    ///
    /// 本项目 build.ps1 会把 CARGO_TARGET_DIR 固定成 `src-tauri\target`，
    /// 但外部 shell 里它可能是别的值（本轮实测是 D:\Projects\.cargo-target），
    /// 所以两种都要能认。
    private fun cargoProfileDir(root: File, triple: String, profile: String): File? {
        val env = System.getenv("CARGO_TARGET_DIR")?.takeIf { it.isNotBlank() }?.let { File(it) }
        val candidates = listOfNotNull(env, File(root, "target"))
        return candidates
            .map { File(File(it, triple), profile) }
            .firstOrNull { it.isDirectory }
    }
}
