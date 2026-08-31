import Foundation

@main
struct WarcraftCompatibilitySupportTests {
    static func main() throws {
        try patchesEveryAffectedOfflineJoinCall()
        try recognizesAnAlreadyFixedWebUI()
        try rejectsAnUnknownWebUI()
        try rejectsUnexpectedSignatureCounts()
        try derivesTheRetailDirectoryFromTheMacApplication()
        try extractsOnlyLoopbackListeningPorts()
        try encodesNicknameConfigurationSafely()
        try validatesRealWebUIFixtureWhenProvided()
        print("WarcraftCompatibilitySupportTests: passed")
    }

    private static func patchesEveryAffectedOfflineJoinCall() throws {
        let buggyCall = "gameId:e.state.selectedGame,"
        let nicknameCall = "localPlayerName:this.props.userInfo.localPlayerName"
        let source = "selectedGameId;"
            + Array(repeating: buggyCall, count: 4).joined()
            + Array(repeating: nicknameCall, count: 7).joined(separator: ";")
        let sourceData = try requiredData(source)

        try require(
            WarcraftCompatibilitySupport.inspectWebUI(sourceData) == .requiresPatch(4),
            "expected four buggy join calls"
        )

        let patch = try WarcraftCompatibilitySupport.patchWebUI(sourceData)
        try require(patch.replacementCount == 11, "expected eleven replacements")
        try require(patch.joinReplacementCount == 4, "expected four join replacements")
        try require(
            patch.nicknameReplacementCount == 7,
            "expected seven nickname replacements"
        )
        try require(
            WarcraftCompatibilitySupport.inspectWebUI(patch.data) == .fixed(4),
            "patched source should be recognized as fixed"
        )
    }

    private static func recognizesAnAlreadyFixedWebUI() throws {
        let fixedJoinCall = "gameId:e.state.selectedGameId,"
        let fixedNicknameCall =
            "localPlayerName:window.strajerNicknameFromConfig()||this.props.userInfo.localPlayerName"
        let sourceData = try requiredData(
            Array(repeating: fixedJoinCall, count: 4).joined()
                + Array(repeating: fixedNicknameCall, count: 7).joined(separator: ";")
                + "window.strajerNicknameFromConfig=function(){return null};"
        )
        try require(
            WarcraftCompatibilitySupport.inspectWebUI(sourceData) == .fixed(4),
            "fixed source should not be patched again"
        )
    }

    private static func rejectsAnUnknownWebUI() throws {
        let sourceData = try requiredData("console.log('unrelated web UI')")
        try require(
            WarcraftCompatibilitySupport.inspectWebUI(sourceData) == .unsupported,
            "unknown source must be rejected"
        )
    }

    private static func rejectsUnexpectedSignatureCounts() throws {
        let buggyJoinCall = "gameId:e.state.selectedGame,"
        let nativeNicknameCall = "localPlayerName:this.props.userInfo.localPlayerName"
        let sourceData = try requiredData(
            "selectedGameId;"
                + Array(repeating: buggyJoinCall, count: 3).joined()
                + Array(repeating: nativeNicknameCall, count: 7).joined(separator: ";")
        )

        try require(
            WarcraftCompatibilitySupport.inspectWebUI(sourceData) == .unsupported,
            "unexpected signature counts must be rejected"
        )
    }

    private static func derivesTheRetailDirectoryFromTheMacApplication() throws {
        let applicationURL = URL(
            fileURLWithPath:
                "/Volumes/Games/Warcraft III/_retail_/x86_64/Warcraft III.app"
        )
        let retailDirectory = try WarcraftCompatibilitySupport.retailDirectory(
            for: applicationURL
        )

        try require(
            retailDirectory.path == "/Volumes/Games/Warcraft III/_retail_",
            "unexpected retail directory"
        )
    }

    private static func extractsOnlyLoopbackListeningPorts() throws {
        let output = """
        p123
        f71
        n127.0.0.1:65190
        f72
        n[::1]:65191
        f78
        n*:16000
        f79
        n192.168.1.3:50938
        f80
        n127.0.0.1:65190
        """

        try require(
            WarcraftCompatibilitySupport.loopbackListeningPorts(fromLsofOutput: output)
                == [65190, 65191],
            "only unique loopback ports should be returned"
        )
    }

    private static func encodesNicknameConfigurationSafely() throws {
        let data = try WarcraftCompatibilitySupport.nicknameConfigurationData("Player#1234")
        try require(
            String(data: data, encoding: .utf8) == #"{"nickname":"Player#1234"}"#,
            "nickname configuration should be deterministic JSON"
        )

        do {
            _ = try WarcraftCompatibilitySupport.nicknameConfigurationData("1234567890123456")
            throw WarcraftCompatibilitySupportTestError.assertionFailed(
                "long nickname should fail"
            )
        } catch WarcraftCompatibilitySupportError.invalidNickname {
        }
    }

    private static func validatesRealWebUIFixtureWhenProvided() throws {
        guard let fixturePath = ProcessInfo.processInfo.environment[
            "STRAJER_GLUE_MANAGER_FIXTURE"
        ] else {
            return
        }

        let fixtureData = try Data(contentsOf: URL(fileURLWithPath: fixturePath))
        try require(
            WarcraftCompatibilitySupport.inspectWebUI(fixtureData) == .requiresPatch(4),
            "real WebUI fixture should contain exactly four affected join calls"
        )

        let patch = try WarcraftCompatibilitySupport.patchWebUI(fixtureData)
        try require(patch.joinReplacementCount == 4, "real WebUI join replacement count")
        try require(patch.nicknameReplacementCount == 7, "real WebUI nickname replacement count")
        try require(
            WarcraftCompatibilitySupport.inspectWebUI(patch.data) == .fixed(4),
            "real WebUI fixture should be fixed after patching"
        )
        try require(
            String(data: patch.data, encoding: .utf8)?.contains("/webui/strajer-config.json")
                == true,
            "patched WebUI should read the Strajer nickname configuration"
        )
    }

    private static func requiredData(_ source: String) throws -> Data {
        guard let data = source.data(using: .utf8) else {
            throw WarcraftCompatibilitySupportTestError.assertionFailed(
                "could not encode test fixture"
            )
        }
        return data
    }

    private static func require(_ condition: Bool, _ message: String) throws {
        guard condition else {
            throw WarcraftCompatibilitySupportTestError.assertionFailed(message)
        }
    }
}

private enum WarcraftCompatibilitySupportTestError: Error {
    case assertionFailed(String)
}
