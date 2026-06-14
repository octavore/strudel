import SwiftUI

@main
struct HelloWorldApp: App {
    var body: some Scene {
        WindowGroup {
            Text("Hello, World!")
                .font(.largeTitle)
                .padding()
                .frame(width: 300, height: 150)
        }
    }
}
