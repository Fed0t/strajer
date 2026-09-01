import Combine
import Foundation
import Network

enum AgentStatus: Equatable {
    case connecting
    case connected
    case reconnecting
    case unavailable

    var description: String {
        switch self {
        case .connecting:
            return "Connecting"
        case .connected:
            return "Connected"
        case .reconnecting:
            return "Reconnecting"
        case .unavailable:
            return "Unavailable"
        }
    }

    var symbolName: String {
        switch self {
        case .connecting:
            return "shield.lefthalf.filled"
        case .connected:
            return "shield.fill"
        case .reconnecting:
            return "shield.lefthalf.filled"
        case .unavailable:
            return "shield.slash"
        }
    }
}

private struct AgentStatusEvent: Decodable {
    let event: String
    let lobbyCount: Int?
    let lobbyID: String?
    let nickname: String?
    let connectionID: UInt64?
    let outcome: String?

    private enum CodingKeys: String, CodingKey {
        case event
        case lobbyCount = "lobby_count"
        case lobbyID = "lobby_id"
        case nickname
        case connectionID = "connection_id"
        case outcome
    }
}

@MainActor
final class AgentController: ObservableObject {
    @Published private(set) var status: AgentStatus = .connecting
    @Published private(set) var availableGames = 0
    @Published private(set) var joinRequestCaptured = false
    @Published private(set) var lobbyJoined = false
    @Published private(set) var lobbyStatusMessage: String?
    @Published private(set) var lastError: String?
    @Published private(set) var retryDelaySeconds: Int?

    private var process: Process?
    private var standardInputPipe: Pipe?
    private var standardOutputPipe: Pipe?
    private var standardErrorPipe: Pipe?
    private var standardOutputBuffer = Data()
    private var standardErrorBuffer = Data()
    private var logFileHandle: FileHandle?
    private var logFileURL: URL?
    private var logFileSize: UInt64 = 0
    private var restartTask: Task<Void, Never>?
    private var networkRestartTask: Task<Void, Never>?
    private var shouldRun = false
    private var restartImmediately = false
    private var consecutiveFailures = 0
    private var networkPathObserved = false
    private var networkAvailable = true
    private var networkMonitorStarted = false
    private var lobbyLifecycle = AgentLobbyLifecycle()
    private let nicknameController: NicknameController
    private let restartPolicy = AgentRestartPolicy()
    private let networkMonitor = NWPathMonitor()
    private let networkMonitorQueue = DispatchQueue(label: "com.clarixpro.strajer.network")
    private let maximumLogFileBytes: UInt64 = 5 * 1_024 * 1_024
    private let retainedLogFileCount = 3

    init(nicknameController: NicknameController) {
        self.nicknameController = nicknameController
    }

    func start() {
        shouldRun = true
        restartTask?.cancel()
        restartTask = nil
        startNetworkMonitoring()

        openLogFileIfNeeded()

        guard process == nil else {
            return
        }

        launchAgent()
    }

    func restart() {
        shouldRun = true
        restartTask?.cancel()
        restartTask = nil
        consecutiveFailures = 0
        retryDelaySeconds = nil

        guard let process else {
            launchAgent()
            return
        }

        restartImmediately = true
        status = .connecting
        availableGames = 0
        resetLobbyLifecycle()
        lastError = nil
        process.terminate()
    }

    func stop() {
        shouldRun = false
        restartImmediately = false
        restartTask?.cancel()
        restartTask = nil
        networkRestartTask?.cancel()
        networkRestartTask = nil
        stopNetworkMonitoring()
        removePipeHandlers()

        process?.terminationHandler = nil
        process?.terminate()
        process = nil
        closeLogFile()
    }

    private func launchAgent() {
        status = .connecting
        availableGames = 0
        resetLobbyLifecycle()
        lastError = nil
        retryDelaySeconds = nil
        standardOutputBuffer.removeAll(keepingCapacity: true)
        standardErrorBuffer.removeAll(keepingCapacity: true)
        appendLogMarker("Launching agent")

        do {
            let agentURL = try resolveAgentURL()
            let newProcess = Process()
            let inputPipe = Pipe()
            let outputPipe = Pipe()
            let errorPipe = Pipe()

            newProcess.executableURL = agentURL
            newProcess.environment = agentEnvironment()
            newProcess.standardInput = inputPipe
            newProcess.standardOutput = outputPipe
            newProcess.standardError = errorPipe
            newProcess.terminationHandler = { @Sendable [weak self] terminatedProcess in
                self?.handleProcessTermination(process: terminatedProcess)
            }

            installPipeHandlers(outputPipe: outputPipe, errorPipe: errorPipe)
            process = newProcess
            standardInputPipe = inputPipe
            standardOutputPipe = outputPipe
            standardErrorPipe = errorPipe
            try newProcess.run()
        } catch {
            process = nil
            removePipeHandlers()
            markUnavailable(error.localizedDescription)
            scheduleRestart()
        }
    }

    private func resolveAgentURL() throws -> URL {
        let agentURL = Bundle.main.bundleURL
            .appendingPathComponent("Contents", isDirectory: true)
            .appendingPathComponent("MacOS", isDirectory: true)
            .appendingPathComponent("strajer-agent", isDirectory: false)

        guard FileManager.default.isExecutableFile(atPath: agentURL.path) else {
            throw AgentControllerError.agentExecutableMissing(agentURL.path)
        }

        return agentURL
    }

    private func agentEnvironment() -> [String: String] {
        var environment = ProcessInfo.processInfo.environment
        environment["RUST_LOG"] = "strajer_agent=info"

        if environment["STRAJER_SERVER_URL"] == nil,
           let serverURL = Bundle.main.object(forInfoDictionaryKey: "StrajerServerURL") as? String {
            environment["STRAJER_SERVER_URL"] = serverURL
        }

        if environment["STRAJER_JOIN_TOKEN"] == nil,
           let joinToken = Bundle.main.object(forInfoDictionaryKey: "StrajerJoinToken") as? String,
           !joinToken.isEmpty {
            environment["STRAJER_JOIN_TOKEN"] = joinToken
        }

        return environment
    }

    private func installPipeHandlers(outputPipe: Pipe, errorPipe: Pipe) {
        outputPipe.fileHandleForReading.readabilityHandler = { @Sendable [weak self] fileHandle in
            self?.handleStandardOutput(fileHandle: fileHandle)
        }
        errorPipe.fileHandleForReading.readabilityHandler = { @Sendable [weak self] fileHandle in
            self?.handleStandardError(fileHandle: fileHandle)
        }
    }

    private func removePipeHandlers() {
        try? standardInputPipe?.fileHandleForWriting.close()
        standardOutputPipe?.fileHandleForReading.readabilityHandler = nil
        standardErrorPipe?.fileHandleForReading.readabilityHandler = nil
        standardInputPipe = nil
        standardOutputPipe = nil
        standardErrorPipe = nil
    }

    nonisolated private func handleStandardOutput(fileHandle: FileHandle) {
        let data = fileHandle.availableData
        guard !data.isEmpty else {
            return
        }

        Task { @MainActor [weak self] in
            self?.consumeStandardOutput(data)
        }
    }

    nonisolated private func handleStandardError(fileHandle: FileHandle) {
        let data = fileHandle.availableData
        guard !data.isEmpty else {
            return
        }

        Task { @MainActor [weak self] in
            self?.consumeStandardError(data)
        }
    }

    nonisolated private func handleProcessTermination(process: Process) {
        Task { @MainActor [weak self] in
            self?.processDidTerminate(process)
        }
    }

    private func consumeStandardOutput(_ data: Data) {
        standardOutputBuffer.append(data)

        while let newlineIndex = standardOutputBuffer.firstIndex(of: 0x0A) {
            let lineData = standardOutputBuffer[..<newlineIndex]
            standardOutputBuffer.removeSubrange(...newlineIndex)
            let statusLine = Data(lineData)
            if consumeStatusLine(statusLine) {
                appendLogMarker("Captured Warcraft nickname")
            } else {
                appendLogLine(statusLine)
            }
        }
    }

    private func consumeStatusLine(_ lineData: Data) -> Bool {
        guard let event = try? JSONDecoder().decode(AgentStatusEvent.self, from: lineData) else {
            return false
        }

        switch event.event {
        case "ready":
            guard let lobbyCount = event.lobbyCount else {
                return false
            }
            status = .connected
            availableGames = lobbyCount
            lastError = nil
            retryDelaySeconds = nil
            consecutiveFailures = 0
        case "join_request_captured":
            guard event.lobbyID != nil, let connectionID = event.connectionID else {
                return false
            }
            if lobbyLifecycle.captureJoinRequest(connectionID: connectionID) {
                lobbyStatusMessage = nil
                publishLobbyLifecycle()
            }
        case "lobby_joined":
            guard event.lobbyID != nil, let connectionID = event.connectionID else {
                return false
            }
            if lobbyLifecycle.markLobbyJoined(connectionID: connectionID) {
                lobbyStatusMessage = nil
                publishLobbyLifecycle()
            }
        case "lobby_session_ended":
            guard event.lobbyID != nil,
                  let connectionID = event.connectionID,
                  let outcome = event.outcome,
                  outcome == "closed" || outcome == "error" else {
                return false
            }
            if lobbyLifecycle.endSession(connectionID: connectionID) {
                lobbyStatusMessage = outcome == "error"
                    ? "Lobby disconnected; rejoin in Warcraft"
                    : nil
                publishLobbyLifecycle()
            }
        case "nickname_captured":
            guard let nickname = event.nickname else {
                return true
            }
            nicknameController.capture(nickname)
            return true
        default:
            return false
        }

        return false
    }

    private func consumeStandardError(_ data: Data) {
        appendLogData(data)
        standardErrorBuffer.append(data)
        let maximumBytes = 8_192
        if standardErrorBuffer.count > maximumBytes {
            standardErrorBuffer.removeFirst(standardErrorBuffer.count - maximumBytes)
        }
    }

    private func processDidTerminate(_ terminatedProcess: Process) {
        guard process === terminatedProcess else {
            return
        }

        process = nil
        removePipeHandlers()

        guard shouldRun else {
            return
        }

        guard networkAvailable else {
            markUnavailable("Network unavailable")
            return
        }

        if restartImmediately {
            restartImmediately = false
            launchAgent()
            return
        }

        if terminatedProcess.terminationReason == .exit,
           terminatedProcess.terminationStatus == 0 {
            consecutiveFailures = 0
            appendLogMarker("Agent requested a controlled restart")
            launchAgent()
            return
        }

        let errorText = latestErrorLine()
            ?? "Agent stopped with exit code \(terminatedProcess.terminationStatus)"
        markUnavailable(errorText)
        scheduleRestart()
    }

    private func latestErrorLine() -> String? {
        guard let output = String(data: standardErrorBuffer, encoding: .utf8) else {
            return nil
        }

        for line in output.split(whereSeparator: isNotNewline).reversed() {
            let value = line.trimmingCharacters(in: .whitespacesAndNewlines)
            if !value.isEmpty {
                return String(value.suffix(180))
            }
        }

        return nil
    }

    private func isNotNewline(character: Character) -> Bool {
        !character.isNewline
    }

    private func markUnavailable(_ message: String) {
        status = .unavailable
        availableGames = 0
        resetLobbyLifecycle()
        lastError = message
    }

    private func resetLobbyLifecycle() {
        lobbyLifecycle.reset()
        lobbyStatusMessage = nil
        publishLobbyLifecycle()
    }

    private func publishLobbyLifecycle() {
        joinRequestCaptured = lobbyLifecycle.joinRequestCaptured
        lobbyJoined = lobbyLifecycle.lobbyJoined
    }

    private func startNetworkMonitoring() {
        guard !networkMonitorStarted else {
            return
        }

        networkMonitorStarted = true
        networkMonitor.pathUpdateHandler = { @Sendable [weak self] path in
            let isAvailable = path.status == .satisfied
            Task { @MainActor [weak self] in
                self?.handleNetworkPathUpdate(isAvailable: isAvailable)
            }
        }
        networkMonitor.start(queue: networkMonitorQueue)
    }

    private func stopNetworkMonitoring() {
        guard networkMonitorStarted else {
            return
        }

        networkMonitor.pathUpdateHandler = nil
        networkMonitor.cancel()
        networkMonitorStarted = false
    }

    private func handleNetworkPathUpdate(isAvailable: Bool) {
        let wasObserved = networkPathObserved
        networkPathObserved = true
        networkAvailable = isAvailable

        guard shouldRun else {
            return
        }

        if !isAvailable {
            restartTask?.cancel()
            restartTask = nil
            networkRestartTask?.cancel()
            networkRestartTask = nil
            restartImmediately = false
            retryDelaySeconds = nil
            markUnavailable("Network unavailable")
            process?.terminate()
            return
        }

        guard wasObserved else {
            return
        }

        scheduleRestartAfterNetworkChange()
    }

    private func scheduleRestartAfterNetworkChange() {
        networkRestartTask?.cancel()
        networkRestartTask = Task { @MainActor [weak self] in
            try? await Task.sleep(for: .seconds(1))
            guard !Task.isCancelled, let self, self.shouldRun, self.networkAvailable else {
                return
            }

            self.networkRestartTask = nil
            self.appendLogMarker("Network path changed; restarting agent")
            self.restart()
        }
    }

    private func openLogFileIfNeeded() {
        guard logFileHandle == nil else {
            return
        }

        do {
            let fileManager = FileManager.default
            let logsDirectory = try fileManager.url(
                for: .libraryDirectory,
                in: .userDomainMask,
                appropriateFor: nil,
                create: true
            )
            .appendingPathComponent("Logs", isDirectory: true)
            .appendingPathComponent("Strajer", isDirectory: true)
            try fileManager.createDirectory(
                at: logsDirectory,
                withIntermediateDirectories: true
            )

            let logFileURL = logsDirectory.appendingPathComponent("agent.log")
            try AgentLogRotation.rotateIfNeeded(
                fileURL: logFileURL,
                maximumBytes: maximumLogFileBytes,
                retainedFileCount: retainedLogFileCount
            )
            if !fileManager.fileExists(atPath: logFileURL.path) {
                guard fileManager.createFile(atPath: logFileURL.path, contents: nil) else {
                    throw AgentControllerError.logFileCreationFailed(logFileURL.path)
                }
            }

            let fileHandle = try FileHandle(forWritingTo: logFileURL)
            try fileHandle.seekToEnd()
            logFileHandle = fileHandle
            self.logFileURL = logFileURL
            let attributes = try fileManager.attributesOfItem(atPath: logFileURL.path)
            logFileSize = (attributes[.size] as? NSNumber)?.uint64Value ?? 0
            appendLogMarker("Strajer started")
        } catch {
            logFileHandle = nil
            self.logFileURL = nil
            logFileSize = 0
        }
    }

    private func appendLogMarker(_ message: String) {
        let timestamp = ISO8601DateFormatter().string(from: Date())
        let marker = "\n[\(timestamp)] \(message)\n"
        guard let data = marker.data(using: .utf8) else {
            return
        }

        appendLogData(data)
    }

    private func appendLogLine(_ data: Data) {
        appendLogData(data)
        appendLogData(Data([0x0A]))
    }

    private func appendLogData(_ data: Data) {
        rotateLogFileIfRequired(incomingByteCount: data.count)
        guard let logFileHandle else {
            return
        }

        do {
            try logFileHandle.write(contentsOf: data)
            logFileSize += UInt64(data.count)
        } catch {
            try? logFileHandle.close()
            self.logFileHandle = nil
            self.logFileURL = nil
            logFileSize = 0
        }
    }

    private func rotateLogFileIfRequired(incomingByteCount: Int) {
        guard incomingByteCount > 0,
              logFileSize > 0,
              logFileSize + UInt64(incomingByteCount) > maximumLogFileBytes,
              let logFileURL else {
            return
        }

        do {
            try logFileHandle?.close()
            logFileHandle = nil
            try AgentLogRotation.rotate(
                fileURL: logFileURL,
                retainedFileCount: retainedLogFileCount
            )
            guard FileManager.default.createFile(atPath: logFileURL.path, contents: nil) else {
                throw AgentControllerError.logFileCreationFailed(logFileURL.path)
            }
            logFileHandle = try FileHandle(forWritingTo: logFileURL)
            logFileSize = 0
        } catch {
            logFileHandle = nil
            self.logFileURL = nil
            logFileSize = 0
        }
    }

    private func closeLogFile() {
        try? logFileHandle?.close()
        logFileHandle = nil
        logFileURL = nil
        logFileSize = 0
    }

    private func scheduleRestart() {
        guard shouldRun, networkAvailable, restartTask == nil else {
            return
        }

        consecutiveFailures += 1
        let jitterFraction = Double.random(in: 0...restartPolicy.maximumJitterFraction)
        let delaySeconds = restartPolicy.delaySeconds(
            forAttempt: consecutiveFailures,
            jitterFraction: jitterFraction
        )
        retryDelaySeconds = Int(ceil(delaySeconds))
        status = .reconnecting
        appendLogMarker("Agent restart scheduled in \(retryDelaySeconds ?? 0) seconds")

        restartTask = Task { @MainActor [weak self] in
            try? await Task.sleep(for: .seconds(delaySeconds))
            guard !Task.isCancelled, let self else {
                return
            }

            self.restartTask = nil
            self.launchAgent()
        }
    }
}

private enum AgentControllerError: LocalizedError {
    case agentExecutableMissing(String)
    case logFileCreationFailed(String)

    var errorDescription: String? {
        switch self {
        case .agentExecutableMissing(let path):
            return "Embedded agent is missing or not executable: \(path)"
        case .logFileCreationFailed(let path):
            return "Could not create the agent log file: \(path)"
        }
    }
}
