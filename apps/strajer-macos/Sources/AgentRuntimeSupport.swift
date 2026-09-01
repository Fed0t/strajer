import Foundation

struct AgentLobbyLifecycle: Equatable {
    private(set) var activeConnectionID: UInt64?
    private(set) var joinRequestCaptured = false
    private(set) var lobbyJoined = false

    mutating func captureJoinRequest(connectionID: UInt64) -> Bool {
        guard connectionID > 0 else {
            return false
        }

        if let activeConnectionID {
            guard connectionID >= activeConnectionID else {
                return false
            }
            if connectionID == activeConnectionID {
                joinRequestCaptured = true
                return true
            }
        }

        activeConnectionID = connectionID
        joinRequestCaptured = true
        lobbyJoined = false
        return true
    }

    mutating func markLobbyJoined(connectionID: UInt64) -> Bool {
        guard activeConnectionID == connectionID else {
            return false
        }

        joinRequestCaptured = true
        lobbyJoined = true
        return true
    }

    mutating func endSession(connectionID: UInt64) -> Bool {
        guard activeConnectionID == connectionID else {
            return false
        }

        reset()
        return true
    }

    mutating func reset() {
        activeConnectionID = nil
        joinRequestCaptured = false
        lobbyJoined = false
    }
}

struct AgentRestartPolicy {
    let initialDelaySeconds: TimeInterval
    let maximumDelaySeconds: TimeInterval
    let maximumJitterFraction: Double

    init(
        initialDelaySeconds: TimeInterval = 2,
        maximumDelaySeconds: TimeInterval = 60,
        maximumJitterFraction: Double = 0.25
    ) {
        precondition(initialDelaySeconds > 0)
        precondition(maximumDelaySeconds >= initialDelaySeconds)
        precondition((0...1).contains(maximumJitterFraction))
        self.initialDelaySeconds = initialDelaySeconds
        self.maximumDelaySeconds = maximumDelaySeconds
        self.maximumJitterFraction = maximumJitterFraction
    }

    func delaySeconds(forAttempt attempt: Int, jitterFraction: Double) -> TimeInterval {
        let normalizedAttempt = max(1, attempt)
        let exponent = min(normalizedAttempt - 1, 20)
        let exponentialDelay = initialDelaySeconds * pow(2, Double(exponent))
        let normalizedJitter = min(max(jitterFraction, 0), maximumJitterFraction)
        return min(exponentialDelay * (1 + normalizedJitter), maximumDelaySeconds)
    }
}

enum AgentLogRotation {
    static func rotateIfNeeded(
        fileURL: URL,
        maximumBytes: UInt64,
        retainedFileCount: Int
    ) throws {
        guard maximumBytes > 0, retainedFileCount > 0 else {
            return
        }

        let attributes = try? FileManager.default.attributesOfItem(atPath: fileURL.path)
        let fileSize = (attributes?[.size] as? NSNumber)?.uint64Value ?? 0
        guard fileSize >= maximumBytes else {
            return
        }

        try rotate(fileURL: fileURL, retainedFileCount: retainedFileCount)
    }

    static func rotate(fileURL: URL, retainedFileCount: Int) throws {
        guard retainedFileCount > 0 else {
            return
        }

        let fileManager = FileManager.default
        for index in stride(from: retainedFileCount, through: 1, by: -1) {
            let destinationURL = rotatedFileURL(fileURL: fileURL, index: index)
            if fileManager.fileExists(atPath: destinationURL.path) {
                try fileManager.removeItem(at: destinationURL)
            }

            let sourceURL = index == 1
                ? fileURL
                : rotatedFileURL(fileURL: fileURL, index: index - 1)
            if fileManager.fileExists(atPath: sourceURL.path) {
                try fileManager.moveItem(at: sourceURL, to: destinationURL)
            }
        }
    }

    static func rotatedFileURL(fileURL: URL, index: Int) -> URL {
        fileURL.appendingPathExtension(String(index))
    }
}
