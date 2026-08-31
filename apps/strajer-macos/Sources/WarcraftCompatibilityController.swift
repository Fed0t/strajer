import AppKit
import Combine
import CoreFoundation
import Foundation

enum WarcraftCompatibilityStatus: Equatable {
    case checking
    case waitingForWarcraft
    case applying
    case ready
    case restartRequired
    case unavailable(String)

    var description: String {
        switch self {
        case .checking:
            return "Offline LAN fix: Checking"
        case .waitingForWarcraft:
            return "Offline LAN fix: Start Warcraft once"
        case .applying:
            return "Offline LAN fix: Applying"
        case .ready:
            return "Offline LAN fix: Ready"
        case .restartRequired:
            return "Offline LAN fix: Restart Warcraft once"
        case .unavailable:
            return "Offline LAN fix: Unavailable"
        }
    }

    var errorDescription: String? {
        guard case .unavailable(let message) = self else {
            return nil
        }
        return message
    }

    var canRetry: Bool {
        if case .unavailable = self {
            return true
        }
        return false
    }
}

@MainActor
final class WarcraftCompatibilityController: ObservableObject, NicknameControllerDelegate {
    @Published private(set) var status: WarcraftCompatibilityStatus = .checking

    private static let minimumWebUIBytes = 1_000_000
    private static let maximumWebUIBytes = 16_000_000
    private static let sourceLookupAttempts = 60
    private static let sourceLookupInterval = Duration.milliseconds(500)

    private let fileManager = FileManager.default
    private let workspace = NSWorkspace.shared
    private var launchObserver: NSObjectProtocol?
    private var evaluationTask: Task<Void, Never>?
    private var shouldRun = false
    private let nicknameController: NicknameController

    init(nicknameController: NicknameController) {
        self.nicknameController = nicknameController
    }

    func start() {
        shouldRun = true
        installLaunchObserverIfNeeded()
        scheduleEvaluation(for: runningWarcraftApplication())
    }

    func retry() {
        scheduleEvaluation(for: runningWarcraftApplication())
    }

    func stop() {
        shouldRun = false
        evaluationTask?.cancel()
        evaluationTask = nil

        if let launchObserver {
            workspace.notificationCenter.removeObserver(launchObserver)
            self.launchObserver = nil
        }
    }

    func nicknameControllerDidChange(_ controller: NicknameController) {
        scheduleEvaluation(for: runningWarcraftApplication())
    }

    private func installLaunchObserverIfNeeded() {
        guard launchObserver == nil else {
            return
        }

        launchObserver = workspace.notificationCenter.addObserver(
            forName: NSWorkspace.didLaunchApplicationNotification,
            object: nil,
            queue: .main
        ) { [weak self] notification in
            guard let application = notification.userInfo?[NSWorkspace.applicationUserInfoKey]
                    as? NSRunningApplication,
                  application.bundleIdentifier
                    == WarcraftCompatibilitySupport.warcraftBundleIdentifier else {
                return
            }

            Task { @MainActor [weak self] in
                self?.scheduleEvaluation(for: application)
            }
        }
    }

    private func runningWarcraftApplication() -> NSRunningApplication? {
        workspace.runningApplications.first(where: isWarcraftApplication)
    }

    private func isWarcraftApplication(_ application: NSRunningApplication) -> Bool {
        application.bundleIdentifier == WarcraftCompatibilitySupport.warcraftBundleIdentifier
            && !application.isTerminated
    }

    private func scheduleEvaluation(for application: NSRunningApplication?) {
        guard shouldRun else {
            return
        }

        evaluationTask?.cancel()
        evaluationTask = Task { @MainActor [weak self] in
            await self?.evaluate(application: application)
        }
    }

    private func evaluate(application: NSRunningApplication?) async {
        status = .checking

        do {
            let applicationURL = try resolveWarcraftApplicationURL(application: application)
            let retailDirectory = try WarcraftCompatibilitySupport.retailDirectory(
                for: applicationURL
            )
            try enableLocalFiles()
            try installNicknameConfiguration(retailDirectory: retailDirectory)

            let overrideURL = webUIOverrideURL(retailDirectory: retailDirectory)
            if fileManager.fileExists(atPath: overrideURL.path) {
                try await evaluateExistingOverride(
                    overrideURL: overrideURL,
                    application: application
                )
                return
            }

            guard let application else {
                status = .waitingForWarcraft
                return
            }

            status = .applying
            let sourceData = try await waitForWebUISource(application: application)
            switch try WarcraftCompatibilitySupport.inspectWebUI(sourceData) {
            case .requiresPatch:
                let patch = try WarcraftCompatibilitySupport.patchWebUI(sourceData)
                try installOverride(patch.data, at: overrideURL)
                status = .restartRequired
            case .fixed:
                status = .ready
            case .unsupported:
                throw WarcraftCompatibilityControllerError.unsupportedWebUI
            }
        } catch is CancellationError {
            return
        } catch {
            status = .unavailable(error.localizedDescription)
        }
    }

    private func evaluateExistingOverride(
        overrideURL: URL,
        application: NSRunningApplication?
    ) async throws {
        try rejectSymbolicLink(at: overrideURL)
        let overrideData = try Data(contentsOf: overrideURL)

        switch try WarcraftCompatibilitySupport.inspectWebUI(overrideData) {
        case .requiresPatch:
            status = .applying
            let patch = try WarcraftCompatibilitySupport.patchWebUI(overrideData)
            try backUpExistingOverrideIfNeeded(overrideURL)
            try installOverride(patch.data, at: overrideURL)
            status = application == nil ? .ready : .restartRequired
        case .fixed:
            guard let application else {
                status = .ready
                return
            }

            do {
                let servedData = try await waitForWebUISource(application: application)
                switch try WarcraftCompatibilitySupport.inspectWebUI(servedData) {
                case .fixed:
                    status = .ready
                case .requiresPatch:
                    status = .restartRequired
                case .unsupported:
                    throw WarcraftCompatibilityControllerError.unsupportedWebUI
                }
            } catch is CancellationError {
                throw CancellationError()
            } catch {
                status = .restartRequired
            }
        case .unsupported:
            throw WarcraftCompatibilityControllerError.conflictingOverride(overrideURL.path)
        }
    }

    private func resolveWarcraftApplicationURL(
        application: NSRunningApplication?
    ) throws -> URL {
        if let applicationURL = application?.bundleURL {
            return applicationURL
        }
        if let applicationURL = workspace.urlForApplication(
            withBundleIdentifier: WarcraftCompatibilitySupport.warcraftBundleIdentifier
        ) {
            return applicationURL
        }

        throw WarcraftCompatibilityControllerError.warcraftNotFound
    }

    private func enableLocalFiles() throws {
        let applicationID = WarcraftCompatibilitySupport.warcraftPreferenceDomain as CFString
        let preferenceKey = WarcraftCompatibilitySupport.localFilesPreferenceKey as CFString

        if let currentValue = CFPreferencesCopyAppValue(preferenceKey, applicationID)
                as? NSNumber,
           currentValue.intValue == 1 {
            return
        }

        CFPreferencesSetAppValue(preferenceKey, NSNumber(value: 1), applicationID)
        guard CFPreferencesAppSynchronize(applicationID) else {
            throw WarcraftCompatibilityControllerError.localFilesPreferenceFailed
        }
    }

    private func webUIOverrideURL(retailDirectory: URL) -> URL {
        retailDirectory
            .appendingPathComponent(
                WarcraftCompatibilitySupport.webUIDirectoryName,
                isDirectory: true
            )
            .appendingPathComponent(
                WarcraftCompatibilitySupport.webUIFileName,
                isDirectory: false
            )
    }

    private func nicknameConfigurationURL(retailDirectory: URL) -> URL {
        retailDirectory
            .appendingPathComponent(
                WarcraftCompatibilitySupport.webUIDirectoryName,
                isDirectory: true
            )
            .appendingPathComponent(
                WarcraftCompatibilitySupport.nicknameConfigurationFileName,
                isDirectory: false
            )
    }

    private func waitForWebUISource(
        application: NSRunningApplication
    ) async throws -> Data {
        let configuration = URLSessionConfiguration.ephemeral
        configuration.timeoutIntervalForRequest = 1
        configuration.timeoutIntervalForResource = 2
        let session = URLSession(configuration: configuration)
        defer {
            session.invalidateAndCancel()
        }

        for _ in 0..<Self.sourceLookupAttempts {
            try Task.checkCancellation()

            let ports = try listeningPorts(processIdentifier: application.processIdentifier)
            for port in ports {
                if let data = await fetchWebUISource(port: port, session: session) {
                    return data
                }
            }

            try await Task.sleep(for: Self.sourceLookupInterval)
        }

        throw WarcraftCompatibilityControllerError.webUISourceUnavailable
    }

    private func listeningPorts(processIdentifier: pid_t) throws -> [UInt16] {
        let process = Process()
        let outputPipe = Pipe()
        process.executableURL = URL(fileURLWithPath: "/usr/sbin/lsof")
        process.arguments = [
            "-nP",
            "-a",
            "-p",
            String(processIdentifier),
            "-iTCP",
            "-sTCP:LISTEN",
            "-Fn",
        ]
        process.standardOutput = outputPipe
        process.standardError = FileHandle.nullDevice

        try process.run()
        let outputData = outputPipe.fileHandleForReading.readDataToEndOfFile()
        process.waitUntilExit()

        guard let output = String(data: outputData, encoding: .utf8) else {
            throw WarcraftCompatibilityControllerError.lsofOutputInvalid
        }

        return WarcraftCompatibilitySupport.loopbackListeningPorts(fromLsofOutput: output)
    }

    private func fetchWebUISource(port: UInt16, session: URLSession) async -> Data? {
        guard let url = URL(
            string: "http://127.0.0.1:\(port)/\(WarcraftCompatibilitySupport.webUIRelativePath)"
        ) else {
            return nil
        }

        do {
            let (data, response) = try await session.data(from: url)
            guard let response = response as? HTTPURLResponse,
                  response.statusCode == 200,
                  data.count >= Self.minimumWebUIBytes,
                  data.count <= Self.maximumWebUIBytes,
                  try WarcraftCompatibilitySupport.inspectWebUI(data) != .unsupported else {
                return nil
            }
            return data
        } catch {
            return nil
        }
    }

    private func backUpExistingOverrideIfNeeded(_ overrideURL: URL) throws {
        let backupURL = overrideURL.appendingPathExtension("strajer-backup")
        guard !fileManager.fileExists(atPath: backupURL.path) else {
            return
        }

        try fileManager.copyItem(at: overrideURL, to: backupURL)
    }

    private func installOverride(_ data: Data, at overrideURL: URL) throws {
        let webUIDirectory = overrideURL.deletingLastPathComponent()
        try rejectSymbolicLink(at: webUIDirectory)
        try rejectSymbolicLink(at: overrideURL)
        try fileManager.createDirectory(
            at: webUIDirectory,
            withIntermediateDirectories: true
        )
        try data.write(to: overrideURL, options: .atomic)
        try fileManager.setAttributes(
            [.posixPermissions: 0o644],
            ofItemAtPath: overrideURL.path
        )

        let installedData = try Data(contentsOf: overrideURL)
        guard installedData == data,
              case .fixed = try WarcraftCompatibilitySupport.inspectWebUI(installedData) else {
            throw WarcraftCompatibilityControllerError.overrideVerificationFailed
        }
    }

    private func installNicknameConfiguration(retailDirectory: URL) throws {
        let configurationURL = nicknameConfigurationURL(retailDirectory: retailDirectory)
        let webUIDirectory = configurationURL.deletingLastPathComponent()
        try rejectSymbolicLink(at: webUIDirectory)
        try rejectSymbolicLink(at: configurationURL)
        try fileManager.createDirectory(
            at: webUIDirectory,
            withIntermediateDirectories: true
        )

        let data = try WarcraftCompatibilitySupport.nicknameConfigurationData(
            nicknameController.nickname
        )
        try data.write(to: configurationURL, options: .atomic)
        try fileManager.setAttributes(
            [.posixPermissions: 0o600],
            ofItemAtPath: configurationURL.path
        )

        guard try Data(contentsOf: configurationURL) == data else {
            throw WarcraftCompatibilityControllerError.nicknameConfigurationVerificationFailed
        }
    }

    private func rejectSymbolicLink(at url: URL) throws {
        guard fileManager.fileExists(atPath: url.path) else {
            return
        }

        let attributes = try fileManager.attributesOfItem(atPath: url.path)
        if attributes[.type] as? FileAttributeType == .typeSymbolicLink {
            throw WarcraftCompatibilityControllerError.symbolicLinkRejected(url.path)
        }
    }
}

private enum WarcraftCompatibilityControllerError: LocalizedError {
    case warcraftNotFound
    case localFilesPreferenceFailed
    case webUISourceUnavailable
    case unsupportedWebUI
    case conflictingOverride(String)
    case lsofOutputInvalid
    case symbolicLinkRejected(String)
    case overrideVerificationFailed
    case nicknameConfigurationVerificationFailed

    var errorDescription: String? {
        switch self {
        case .warcraftNotFound:
            return "Warcraft III is not installed or registered with macOS"
        case .localFilesPreferenceFailed:
            return "Could not enable Warcraft Allow Local Files"
        case .webUISourceUnavailable:
            return "Could not read GlueManager.js from the running Warcraft instance"
        case .unsupportedWebUI:
            return "This Warcraft WebUI version is not supported safely"
        case .conflictingOverride(let path):
            return "An unsupported WebUI override already exists at \(path)"
        case .lsofOutputInvalid:
            return "Could not inspect the Warcraft WebUI listener"
        case .symbolicLinkRejected(let path):
            return "Refusing to write through a symbolic link at \(path)"
        case .overrideVerificationFailed:
            return "The installed Warcraft WebUI override failed verification"
        case .nicknameConfigurationVerificationFailed:
            return "The Warcraft nickname configuration failed verification"
        }
    }
}
