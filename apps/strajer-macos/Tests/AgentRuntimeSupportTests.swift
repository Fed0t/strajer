import Foundation

@main
struct AgentRuntimeSupportTests {
    static func main() throws {
        try correlatesLobbyLifecycleEvents()
        try calculatesBoundedExponentialBackoff()
        try rotatesAndBoundsAgentLogs()
        print("AgentRuntimeSupportTests: passed")
    }

    private static func correlatesLobbyLifecycleEvents() throws {
        var lifecycle = AgentLobbyLifecycle()

        try require(
            lifecycle.captureJoinRequest(connectionID: 41),
            "first join request should become active"
        )
        try require(
            lifecycle.markLobbyJoined(connectionID: 41),
            "active connection should reach joined state"
        )
        try require(lifecycle.lobbyJoined, "active lobby should be marked joined")

        try require(
            lifecycle.captureJoinRequest(connectionID: 42),
            "newer join request should replace the active connection"
        )
        try require(!lifecycle.lobbyJoined, "new join request should clear stale joined state")
        try require(
            !lifecycle.endSession(connectionID: 41),
            "delayed end from an older connection must be ignored"
        )
        try require(
            lifecycle.markLobbyJoined(connectionID: 42),
            "newest connection should reach joined state"
        )
        try require(
            lifecycle.endSession(connectionID: 42),
            "active connection end should clear lobby state"
        )
        try require(
            lifecycle.activeConnectionID == nil
                && !lifecycle.joinRequestCaptured
                && !lifecycle.lobbyJoined,
            "ended session should leave no stale lobby state"
        )
    }

    private static func calculatesBoundedExponentialBackoff() throws {
        let policy = AgentRestartPolicy()
        let expectedDelays: [TimeInterval] = [2, 4, 8, 16, 32, 60, 60]
        let actualDelays = (1...7).map { attempt in
            policy.delaySeconds(forAttempt: attempt, jitterFraction: 0)
        }
        try require(actualDelays == expectedDelays, "unexpected exponential backoff sequence")
        try require(
            policy.delaySeconds(forAttempt: 1, jitterFraction: 0.25) == 2.5,
            "first retry should include deterministic jitter"
        )
        try require(
            policy.delaySeconds(forAttempt: 20, jitterFraction: 0.25) == 60,
            "retry delay must remain bounded"
        )
    }

    private static func rotatesAndBoundsAgentLogs() throws {
        let fileManager = FileManager.default
        let directoryURL = fileManager.temporaryDirectory
            .appendingPathComponent("strajer-agent-log-tests-\(UUID().uuidString)", isDirectory: true)
        try fileManager.createDirectory(at: directoryURL, withIntermediateDirectories: true)
        defer {
            try? fileManager.removeItem(at: directoryURL)
        }

        let logURL = directoryURL.appendingPathComponent("agent.log")
        let firstBackupURL = AgentLogRotation.rotatedFileURL(fileURL: logURL, index: 1)
        let secondBackupURL = AgentLogRotation.rotatedFileURL(fileURL: logURL, index: 2)
        try Data("current".utf8).write(to: logURL)
        try Data("previous-1".utf8).write(to: firstBackupURL)
        try Data("previous-2".utf8).write(to: secondBackupURL)

        try AgentLogRotation.rotateIfNeeded(
            fileURL: logURL,
            maximumBytes: 7,
            retainedFileCount: 2
        )

        try require(!fileManager.fileExists(atPath: logURL.path), "active log should rotate")
        try require(
            try String(contentsOf: firstBackupURL, encoding: .utf8) == "current",
            "newest backup should contain the active log"
        )
        try require(
            try String(contentsOf: secondBackupURL, encoding: .utf8) == "previous-1",
            "older backup should shift exactly once"
        )
    }

    private static func require(_ condition: Bool, _ message: String) throws {
        guard condition else {
            throw AgentRuntimeSupportTestError.assertionFailed(message)
        }
    }
}

private enum AgentRuntimeSupportTestError: Error {
    case assertionFailed(String)
}
