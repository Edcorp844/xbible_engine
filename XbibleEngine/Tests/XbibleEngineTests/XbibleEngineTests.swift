import Testing
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
