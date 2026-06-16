import SwiftUI

struct MultiTargetAppMobile: App {
    var body: some Scene {
        WindowGroup {
            ContentView()
        }
    }
}

struct ContentView: View {
    var body: some View {
        VStack(spacing: 16) {
            Image(systemName: "iphone")
                .imageScale(.large)
                .foregroundStyle(.tint)
            Text("MultiTargetApp — iOS")
                .font(.title2)
        }
        .padding()
    }
}

MultiTargetAppMobile.main()
