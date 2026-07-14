package com.github.simontreanor.pyfun

import com.intellij.openapi.fileTypes.LanguageFileType
import com.intellij.openapi.util.IconLoader
import javax.swing.Icon

object PyfunFileType : LanguageFileType(PyfunLanguage) {
    override fun getName(): String = "Pyfun"

    override fun getDescription(): String = "Pyfun source file"

    override fun getDefaultExtension(): String = "pyfun"

    override fun getIcon(): Icon =
        IconLoader.getIcon("/icons/pyfun.svg", PyfunFileType::class.java)
}
