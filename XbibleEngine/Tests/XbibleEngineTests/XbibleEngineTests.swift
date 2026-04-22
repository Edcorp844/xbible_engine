import Testing
import Foundation
@testable import XbibleEngine

@Test func example() async throws {
    // Test the complete Bible API with module management

    // Create a new BibleEngine instance
    let bibleEngine = BibleEngine()

    // Get available Bible modules
    let modules = bibleEngine.getAvailableModules()
    print("Available Bible modules: \(modules.map { $0.name })")

    // Find KJV module
    guard let kjvModule = modules.first(where: { $0.name == "KJV" }) else {
        print("KJV module not found")
        return
    }

    print("Using module: \(kjvModule.name) - \(kjvModule.description)")

    // Get the book structure for KJV
    let books = bibleEngine.getBooks(moduleName: kjvModule.name)
    print("Books in \(kjvModule.name): \(books.map { $0.name })")

    // Get content for John 3:16
    let content = bibleEngine.getContent(moduleName: kjvModule.name, reference: "John 3:16")
    print("Content for John 3:16:")
    for section in content {
        print("Title: \(section.title.map { $0.text }.joined())")
        for verse in section.verses {
            print("Verse \(verse.number): \(verse.words.map { $0.text }.joined())")
        }
    }

    // Get chapter content for John 3
    let chapterContent = bibleEngine.getChapterContent(moduleName: kjvModule.name, reference: "John 3")
    print("Chapter content for John 3 has \(chapterContent.count) sections")

    // Test module management API
    print("\n--- Module Management API ---")

    // Get remote sources with details
    let sources = bibleEngine.getRemoteSourcesWithDetails()
    print("Available remote sources:")
    for source in sources {
        print("  - \(source.name): \(source.description)")
    }

    // Check if modules are installed
    print("\nModule installation status:")
    print("  KJV installed: \(bibleEngine.isModuleInstalled(moduleName: "KJV"))")
    print("  NASB installed: \(bibleEngine.isModuleInstalled(moduleName: "NASB"))")

    // Get download progress details
    let progress = bibleEngine.getDownloadProgressDetails()
    print("\nDownload Progress:")
    print("  Status: \(progress.status)")
    print("  Progress: \(String(format: "%.1f", progress.progress * 100))%")
    print("  Downloaded: \(progress.downloadedBytes) / \(progress.totalBytes) bytes")

    // Get installed modules size
    let totalSize = bibleEngine.getInstalledModulesSize()
    print("Total installed modules size: \(totalSize) bytes")

    // Get installed Bible modules
    let installedBibles = bibleEngine.getInstalledModulesByCategory(category: "Biblical Texts")
    print("Installed Bible modules: \(installedBibles.map { $0.name })")

    // Try to search for remote modules from CrossWire
    print("\nSearching for remote modules...")
    if let firstSource = sources.first {
        let remoteModules = bibleEngine.fetchRemoteModules(sourceName: firstSource.name)
        print("Found \(remoteModules.count) modules from \(firstSource.name)")

        // Search for specific modules
        let nasb = bibleEngine.getRemoteModuleInfo(sourceName: firstSource.name, moduleName: "NASB")
        if let nasbModule = nasb {
            print("Found NASB: \(nasbModule.description)")
        }

        // Search modules by query
        let englishModules = bibleEngine.searchModules(sourceName: firstSource.name, query: "english")
        print("English modules found: \(englishModules.count)")

        // Get modules by language
        let hebrewModules = bibleEngine.getModulesByLanguage(languageCode: "he", sourceName: firstSource.name)
        print("Hebrew modules found: \(hebrewModules.count)")
    }

    // Test other module types
    let commentaries = bibleEngine.getCommentaryModules()
    print("\nAvailable commentaries: \(commentaries.map { $0.name })")

    let dictionaries = bibleEngine.getDictionaryModules()
    print("Available dictionaries: \(dictionaries.map { $0.name })")
}

@Test func testModuleInstallationProgress() async throws {
    // Test real-time progress tracking during module installation

    let bibleEngine = BibleEngine()

    // Get remote sources
    let sources = bibleEngine.getRemoteSourcesWithDetails()
    guard let firstSource = sources.first else {
        print("No remote sources available")
        return
    }

    print("Using source: \(firstSource.name)")

    // Fetch available modules from the first source
    let remoteModules = bibleEngine.fetchRemoteModules(sourceName: firstSource.name)
    guard !remoteModules.isEmpty else {
        print("No remote modules available")
        return
    }

    // Find a small module to install (preferably a commentary or dictionary, not a full Bible)
    let smallModules = remoteModules.filter { module in
        // Look for small modules like commentaries or dictionaries
        module.category.lowercased().contains("comment") ||
        module.category.lowercased().contains("dict") ||
        module.name.lowercased().contains("notes") ||
        module.name.lowercased().contains("study")
    }

    let moduleToInstall = smallModules.first ?? remoteModules.first!
    print("Selected module for installation: \(moduleToInstall.name) - \(moduleToInstall.description)")
    print("Category: \(moduleToInstall.category), Size estimate: ~5MB")

    // Check if already installed
    if bibleEngine.isModuleInstalled(moduleName: moduleToInstall.name) {
        print("Module \(moduleToInstall.name) is already installed")
        return
    }

    print("\n--- Starting Installation ---")
    print("Installing \(moduleToInstall.name) from \(firstSource.name)...")

    // Start installation in background
    Task {
        let result = bibleEngine.installModuleWithProgress(source: firstSource.name, moduleName: moduleToInstall.name)
        print("\nInstallation completed with result: \(result)")
    }

    // Monitor progress for up to 5 minutes
    let startTime = Date()
    var lastProgress: Double = -1
    var lastStatus = ""

    while true {
        let progress = bibleEngine.getDownloadProgressDetails()
        let elapsed = Date().timeIntervalSince(startTime)

        // Only print if progress or status changed
        if progress.progress != lastProgress || progress.status != lastStatus {
            print(String(format: "[%.1fs] Progress: %.1f%% - Status: \(progress.status) - Downloaded: \(progress.downloadedBytes)/\(progress.totalBytes) bytes",
                         elapsed, progress.progress * 100))

            lastProgress = progress.progress
            lastStatus = progress.status
        }

        // Check if installation is complete
        if progress.progress >= 1.0 || elapsed > 300 { // 5 minutes timeout
            break
        }

        // Wait 1 second before checking again
        try? await Task.sleep(nanoseconds: 1_000_000_000)
    }

    print("\n--- Installation Complete ---")

    // Verify installation
    let isInstalled = bibleEngine.isModuleInstalled(moduleName: moduleToInstall.name)
    print("Module \(moduleToInstall.name) installed: \(isInstalled)")

    // Show final installed modules count
    let installedModules = bibleEngine.getAvailableModules()
    print("Total installed modules: \(installedModules.count)")
}
