import Foundation

enum WarcraftWebUIPatchState: Equatable {
    case requiresPatch(Int)
    case fixed(Int)
    case unsupported
}

struct WarcraftWebUIPatch {
    let data: Data
    let replacementCount: Int
    let joinReplacementCount: Int
    let nicknameReplacementCount: Int
}

enum WarcraftCompatibilitySupportError: LocalizedError {
    case invalidWarcraftApplicationPath(String)
    case invalidWebUIEncoding
    case unsupportedWebUI
    case invalidNickname

    var errorDescription: String? {
        switch self {
        case .invalidWarcraftApplicationPath(let path):
            return "Unsupported Warcraft III application path: \(path)"
        case .invalidWebUIEncoding:
            return "Warcraft GlueManager.js is not valid UTF-8"
        case .unsupportedWebUI:
            return "This Warcraft GlueManager.js version cannot be patched safely"
        case .invalidNickname:
            return "Nickname must contain 1 to 15 UTF-8 bytes and no control characters"
        }
    }
}

struct WarcraftCompatibilitySupport {
    static let warcraftBundleIdentifier = "com.blizzard.WarcraftIII"
    static let warcraftPreferenceDomain = "com.blizzard.Warcraft III"
    static let localFilesPreferenceKey = "Allow Local Files"
    static let webUIDirectoryName = "webui"
    static let webUIFileName = "GlueManager.js"
    static let webUIRelativePath = "webui/GlueManager.js"
    static let nicknameConfigurationFileName = "strajer-config.json"

    private static let buggyJoinExpression = "gameId:e.state.selectedGame,"
    private static let fixedJoinExpression = "gameId:e.state.selectedGameId,"
    private static let expectedJoinExpressionCount = 4
    private static let selectedGameIDStateMarker = "selectedGameId"
    private static let nativeNicknameExpression =
        "localPlayerName:this.props.userInfo.localPlayerName"
    private static let strajerNicknameExpression =
        "localPlayerName:window.strajerNicknameFromConfig()||this.props.userInfo.localPlayerName"
    private static let expectedNicknameExpressionCount = 7
    private static let nicknameHelperMarker = "window.strajerNicknameFromConfig="
    private static let nicknameHelperSource =
        "window.strajerNicknameFromConfig=window.strajerNicknameFromConfig||function(){try{var e=new XMLHttpRequest;e.open(\"GET\",\"/webui/strajer-config.json\",false);e.send(null);if(e.status!==200){return null}var n=JSON.parse(e.responseText);return typeof n.nickname===\"string\"&&n.nickname.length>0?n.nickname:null}catch(e){return null}};"

    static func inspectWebUI(_ data: Data) throws -> WarcraftWebUIPatchState {
        guard let source = String(data: data, encoding: .utf8) else {
            throw WarcraftCompatibilitySupportError.invalidWebUIEncoding
        }

        let buggyCount = occurrenceCount(of: buggyJoinExpression, in: source)
        let fixedCount = occurrenceCount(of: fixedJoinExpression, in: source)
        let nativeNicknameCount = occurrenceCount(of: nativeNicknameExpression, in: source)
        let strajerNicknameCount = occurrenceCount(of: strajerNicknameExpression, in: source)
        let nicknameHelperCount = occurrenceCount(of: nicknameHelperMarker, in: source)

        let joinCallCount: Int
        let joinRequiresPatch: Bool
        if buggyCount == expectedJoinExpressionCount
            && fixedCount == 0
            && source.contains(selectedGameIDStateMarker) {
            joinCallCount = buggyCount
            joinRequiresPatch = true
        } else if buggyCount == 0 && fixedCount == expectedJoinExpressionCount {
            joinCallCount = fixedCount
            joinRequiresPatch = false
        } else {
            return .unsupported
        }

        let nicknameRequiresPatch = nativeNicknameCount == expectedNicknameExpressionCount
            && strajerNicknameCount == 0
            && nicknameHelperCount == 0
        let nicknameIsFixed = nativeNicknameCount == 0
            && strajerNicknameCount == expectedNicknameExpressionCount
            && nicknameHelperCount == 1

        if joinRequiresPatch || nicknameRequiresPatch {
            guard nicknameRequiresPatch || nicknameIsFixed else {
                return .unsupported
            }
            return .requiresPatch(joinCallCount)
        }
        if nicknameIsFixed {
            return .fixed(joinCallCount)
        }

        return .unsupported
    }

    static func patchWebUI(_ data: Data) throws -> WarcraftWebUIPatch {
        guard let source = String(data: data, encoding: .utf8) else {
            throw WarcraftCompatibilitySupportError.invalidWebUIEncoding
        }
        guard case .requiresPatch = try inspectWebUI(data) else {
            throw WarcraftCompatibilitySupportError.unsupportedWebUI
        }

        let joinReplacementCount = occurrenceCount(of: buggyJoinExpression, in: source)
        let nicknameReplacementCount = occurrenceCount(of: nativeNicknameExpression, in: source)
        var patchedSource = source.replacingOccurrences(
            of: buggyJoinExpression,
            with: fixedJoinExpression
        )
        patchedSource = patchedSource.replacingOccurrences(
            of: nativeNicknameExpression,
            with: strajerNicknameExpression
        )
        if !patchedSource.contains(nicknameHelperMarker) {
            patchedSource.append("\n")
            patchedSource.append(nicknameHelperSource)
            patchedSource.append("\n")
        }
        guard let patchedData = patchedSource.data(using: .utf8),
              case .fixed = try inspectWebUI(patchedData) else {
            throw WarcraftCompatibilitySupportError.unsupportedWebUI
        }

        return WarcraftWebUIPatch(
            data: patchedData,
            replacementCount: joinReplacementCount + nicknameReplacementCount,
            joinReplacementCount: joinReplacementCount,
            nicknameReplacementCount: nicknameReplacementCount
        )
    }

    static func nicknameConfigurationData(_ nickname: String?) throws -> Data {
        if let nickname,
           nickname.isEmpty
                || nickname.lengthOfBytes(using: .utf8) > 15
                || nickname.contains("\0")
                || nickname.unicodeScalars.contains(where: isControlCharacter) {
            throw WarcraftCompatibilitySupportError.invalidNickname
        }

        let encoder = JSONEncoder()
        encoder.outputFormatting = [.sortedKeys]
        return try encoder.encode(WarcraftNicknameConfiguration(nickname: nickname))
    }

    static func retailDirectory(for applicationURL: URL) throws -> URL {
        let architectureDirectory = applicationURL.deletingLastPathComponent()
        let retailDirectory = architectureDirectory.deletingLastPathComponent()

        guard applicationURL.lastPathComponent == "Warcraft III.app",
              architectureDirectory.lastPathComponent == "x86_64",
              retailDirectory.lastPathComponent == "_retail_" else {
            throw WarcraftCompatibilitySupportError.invalidWarcraftApplicationPath(
                applicationURL.path
            )
        }

        return retailDirectory
    }

    static func loopbackListeningPorts(fromLsofOutput output: String) -> [UInt16] {
        var ports = Set<UInt16>()

        for line in output.split(whereSeparator: isNewline) {
            guard line.first == "n" else {
                continue
            }

            let endpoint = line.dropFirst()
            let portText: Substring
            if endpoint.hasPrefix("127.0.0.1:") || endpoint.hasPrefix("localhost:") {
                guard let separator = endpoint.lastIndex(of: ":") else {
                    continue
                }
                portText = endpoint[endpoint.index(after: separator)...]
            } else if endpoint.hasPrefix("[::1]:") {
                guard let separator = endpoint.lastIndex(of: ":") else {
                    continue
                }
                portText = endpoint[endpoint.index(after: separator)...]
            } else {
                continue
            }

            guard let port = UInt16(portText), port > 0 else {
                continue
            }
            ports.insert(port)
        }

        return ports.sorted()
    }

    private static func occurrenceCount(of needle: String, in source: String) -> Int {
        var count = 0
        var searchStart = source.startIndex

        while searchStart < source.endIndex,
              let range = source.range(
                  of: needle,
                  range: searchStart..<source.endIndex
              ) {
            count += 1
            searchStart = range.upperBound
        }

        return count
    }

    private static func isNewline(character: Character) -> Bool {
        character.isNewline
    }

    private static func isControlCharacter(_ scalar: UnicodeScalar) -> Bool {
        CharacterSet.controlCharacters.contains(scalar)
    }
}

private struct WarcraftNicknameConfiguration: Encodable {
    let nickname: String?
}
