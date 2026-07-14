package com.github.simontreanor.pyfun

import com.intellij.openapi.diagnostic.Logger
import java.nio.file.Files
import java.nio.file.Path
import org.jetbrains.plugins.textmate.api.TextMateBundleProvider

/**
 * Serves the VS Code-format TextMate bundle (grammar + language configuration)
 * packaged under resources/textmate/pyfun. The TextMate engine wants a
 * filesystem path, so the resources are extracted to a temp directory once per
 * session.
 */
class PyfunTextMateBundleProvider : TextMateBundleProvider {
    private val log = Logger.getInstance(PyfunTextMateBundleProvider::class.java)

    override fun getBundles(): List<TextMateBundleProvider.PluginBundle> {
        return try {
            val dir = extractBundle()
            listOf(TextMateBundleProvider.PluginBundle("Pyfun", dir))
        } catch (e: Exception) {
            log.warn("Failed to provide the Pyfun TextMate bundle", e)
            emptyList()
        }
    }

    private fun extractBundle(): Path {
        val dir = Files.createTempDirectory("pyfun-textmate")
        dir.toFile().deleteOnExit()
        for (name in listOf("package.json", "pyfun.tmLanguage.json", "language-configuration.json")) {
            val resource = "/textmate/pyfun/$name"
            javaClass.getResourceAsStream(resource)?.use { input ->
                Files.copy(input, dir.resolve(name))
            } ?: error("missing plugin resource $resource")
        }
        return dir
    }
}
