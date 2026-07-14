// JetBrains plugin for Pyfun: file type + LSP (via LSP4IJ) + TextMate highlighting.
// Thin by design — the language server does the real work. Built with the
// IntelliJ Platform Gradle Plugin 2.x; targets 2024.2+ (LSP4IJ's floor), which
// includes unified PyCharm free mode and the legacy Community editions.

plugins {
    id("java")
    kotlin("jvm") version "2.0.21"
    id("org.jetbrains.intellij.platform") version "2.2.1"
}

group = "com.github.simontreanor"
version = "0.1.0"

repositories {
    mavenCentral()
    intellijPlatform {
        defaultRepositories()
    }
}

dependencies {
    intellijPlatform {
        intellijIdeaCommunity("2024.2.4")
        plugin("com.redhat.devtools.lsp4ij:0.20.1")
        bundledPlugin("org.jetbrains.plugins.textmate")
    }
}

kotlin {
    jvmToolchain(21)
}

intellijPlatform {
    buildSearchableOptions = false
    pluginConfiguration {
        id = "com.github.simontreanor.pyfun"
        name = "Pyfun"
        version = project.version.toString()
        description = """
            <p>Language support for <a href="https://github.com/simontreanor/Pyfun">Pyfun</a>,
            an F#-inspired, functional-first language that compiles to readable Python.</p>
            <p>Diagnostics, hover types & effects, go-to-definition, rename, and completion via the
            bundled <code>pyfun lsp</code> language server (install with
            <code>pip install pyfun-lang</code>), plus TextMate syntax highlighting.</p>
        """.trimIndent()
        vendor {
            name = "Simon Treanor"
            url = "https://github.com/simontreanor/Pyfun"
        }
        ideaVersion {
            sinceBuild = "242"
            untilBuild = provider { null }
        }
    }
    publishing {
        token = providers.environmentVariable("JETBRAINS_PERMANENT_TOKEN")
    }
}

// The TextMate bundle ships from the VS Code extension directory — single
// source of truth for the grammar.
tasks.processResources {
    from("../vscode") {
        include("package.json", "pyfun.tmLanguage.json", "language-configuration.json")
        into("textmate/pyfun")
    }
}
