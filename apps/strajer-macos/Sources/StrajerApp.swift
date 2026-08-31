import AppKit
import SwiftUI

@main
struct StrajerApp: App {
    @NSApplicationDelegateAdaptor(StrajerAppDelegate.self)
    private var appDelegate

    var body: some Scene {
        MenuBarExtra {
            StrajerMenuView(
                controller: appDelegate.agentController,
                compatibilityController: appDelegate.compatibilityController,
                nicknameController: appDelegate.nicknameController
            )
        } label: {
            StrajerMenuBarLabel(
                controller: appDelegate.agentController,
                compatibilityController: appDelegate.compatibilityController
            )
        }
        .menuBarExtraStyle(.menu)
    }
}

@MainActor
final class StrajerAppDelegate: NSObject, NSApplicationDelegate {
    let nicknameController: NicknameController
    let agentController: AgentController
    let compatibilityController: WarcraftCompatibilityController

    override init() {
        let nicknameController = NicknameController()
        self.nicknameController = nicknameController
        agentController = AgentController(nicknameController: nicknameController)
        compatibilityController = WarcraftCompatibilityController(
            nicknameController: nicknameController
        )
        super.init()
        nicknameController.delegate = compatibilityController
    }

    func applicationDidFinishLaunching(_ notification: Notification) {
        compatibilityController.start()
        agentController.start()
    }

    func applicationWillTerminate(_ notification: Notification) {
        agentController.stop()
        compatibilityController.stop()
    }
}

private struct StrajerMenuBarLabel: View {
    @ObservedObject var controller: AgentController
    @ObservedObject var compatibilityController: WarcraftCompatibilityController

    var body: some View {
        Image(systemName: controller.status.symbolName)
            .accessibilityLabel("Strajer")
            .help(
                "\(controller.status.description); "
                    + compatibilityController.status.description
            )
    }
}

private struct StrajerMenuView: View {
    @ObservedObject var controller: AgentController
    @ObservedObject var compatibilityController: WarcraftCompatibilityController
    @ObservedObject var nicknameController: NicknameController

    var body: some View {
        Text("Strajer")
            .font(.headline)

        Button("Nickname...") {
            nicknameController.promptForNickname()
        }

        Text("Nickname: \(nicknameController.menuDescription)")

        Text(controller.status.description)

        if controller.status == .connected {
            Text(gameCountLabel)

            if controller.lobbyJoined {
                Text("Lobby joined")
            } else if controller.joinRequestCaptured {
                Text("Join request detected")
            }
        }

        if let lastError = controller.lastError, controller.status == .unavailable {
            Text(lastError)
                .lineLimit(2)
        }

        Divider()

        Text(compatibilityController.status.description)

        if let compatibilityError = compatibilityController.status.errorDescription {
            Text(compatibilityError)
                .lineLimit(3)
        }

        if compatibilityController.status.canRetry {
            Button("Retry Offline LAN Fix") {
                compatibilityController.retry()
            }
        }

        Divider()

        Button("Restart Agent") {
            controller.restart()
        }
        .keyboardShortcut("r")

        Divider()

        Button("Quit Strajer") {
            NSApplication.shared.terminate(nil)
        }
        .keyboardShortcut("q")
    }

    private var gameCountLabel: String {
        if controller.availableGames == 1 {
            return "1 game available"
        }

        return "\(controller.availableGames) games available"
    }
}
