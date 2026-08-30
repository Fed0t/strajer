import Combine
import Foundation

enum AgentStatus: Equatable {
    case connecting
    case connected
    case unavailable

    var description: String {
        switch self {
        case .connecting:
            return "Connecting"
        case .connected:
            return "Connected"
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
        case .unavailable:
            return "shield.slash"
        }
    }
}

private struct AgentStatusEvent: Decodable {
    let event: String
    let lobbyCount: Int?
    let lobbyID: String?

    private enum CodingKeys: String, CodingKey {
        case event
        case lobbyCount = "lobby_count"
        case lobbyID = "lobby_id"
    }
}

@MainActor
final class AgentController: ObservableObject {
    @Published private(set) var status: AgentStatus = .connecting
    @Published private(set) var availableGames = 0
    @Published private(set) var joinRequestCaptured = false
    @Published private(set) var lastError: String?

    private var process: Process?
    private var standardInputPipe: Pipe?
    private var standardOutputPipe: Pipe?
    private var standardErrorPipe: Pipe?
    private var standardOutputBuffer = Data()
    private var standardErrorBuffer = Data()
    private var restartTask: Task<Void, Never>?
    private var shouldRun = false
    private var restartImmediately = false

    func start() {
        shouldRun = true
        restartTask?.cancel()
        restartTask = nil

        guard process == nil else {
            return
        }

        launchAgent()
    }

    func restart() {
        shouldRun = true
        restartTask?.cancel()
        restartTask = nil

        guard let process else {
            launchAgent()
            return
        }

        restartImmediately = true
        status = .connecting
        availableGames = 0
        joinRequestCaptured = false
        lastError = nil
        process.terminate()
    }

    func stop() {
        shouldRun = false
        restartImmediately = false
        restartTask?.cancel()
        restartTask = nil
        removePipeHandlers()

        process?.terminationHandler = nil
        process?.terminate()
        process = nil
    }

    private func launchAgent() {
        status = .connecting
        availableGames = 0
        joinRequestCaptured = false
        lastError = nil
        standardOutputBuffer.removeAll(keepingCapacity: true)
        standardErrorBuffer.removeAll(keepingCapacity: true)

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
            consumeStatusLine(Data(lineData))
        }
    }

    private func consumeStatusLine(_ lineData: Data) {
        guard let event = try? JSONDecoder().decode(AgentStatusEvent.self, from: lineData) else {
            return
        }

        switch event.event {
        case "ready":
            guard let lobbyCount = event.lobbyCount else {
                return
            }
            status = .connected
            availableGames = lobbyCount
            lastError = nil
        case "join_request_captured":
            guard event.lobbyID != nil else {
                return
            }
            joinRequestCaptured = true
        default:
            return
        }
    }

    private func consumeStandardError(_ data: Data) {
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

        if restartImmediately {
            restartImmediately = false
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
        joinRequestCaptured = false
        lastError = message
    }

    private func scheduleRestart() {
        guard shouldRun, restartTask == nil else {
            return
        }

        restartTask = Task { @MainActor [weak self] in
            try? await Task.sleep(for: .seconds(5))
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

    var errorDescription: String? {
        switch self {
        case .agentExecutableMissing(let path):
            return "Embedded agent is missing or not executable: \(path)"
        }
    }
}
