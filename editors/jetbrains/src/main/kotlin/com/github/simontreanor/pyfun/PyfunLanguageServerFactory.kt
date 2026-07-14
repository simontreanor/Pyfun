package com.github.simontreanor.pyfun

import com.intellij.openapi.project.Project
import com.redhat.devtools.lsp4ij.LanguageServerFactory
import com.redhat.devtools.lsp4ij.server.ProcessStreamConnectionProvider
import com.redhat.devtools.lsp4ij.server.StreamConnectionProvider
import java.io.File

class PyfunLanguageServerFactory : LanguageServerFactory {
    override fun createConnectionProvider(project: Project): StreamConnectionProvider =
        PyfunConnectionProvider()
}

/** Launches `pyfun lsp` (stdio). Resolution order: PYFUN_BIN, then PATH. */
class PyfunConnectionProvider : ProcessStreamConnectionProvider() {
    init {
        commands = listOf(findPyfun(), "lsp")
    }

    private fun findPyfun(): String {
        System.getenv("PYFUN_BIN")?.let { if (it.isNotBlank()) return it }
        val exe = if (System.getProperty("os.name").startsWith("Windows")) "pyfun.exe" else "pyfun"
        System.getenv("PATH")?.split(File.pathSeparator)?.forEach { dir ->
            val candidate = File(dir.trim(), exe)
            if (candidate.isFile && candidate.canExecute()) return candidate.absolutePath
        }
        // Fall through to a bare name: the failure then surfaces in LSP4IJ's
        // console with a comprehensible "cannot run pyfun" message.
        return "pyfun"
    }
}
