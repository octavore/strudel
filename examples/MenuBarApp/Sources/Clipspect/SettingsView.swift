import SwiftUI

struct SettingsView: View {
  var body: some View {
    TabView {
      GeneralSettingsView()
        .tabItem {
          Label("General", systemImage: "gearshape")
        }
        .navigationTitle("Chevre")
    }
    .padding(20)
    .frame(width: 400, height: 300, alignment: .top)
  }
}

struct GeneralSettingsView: View {
  var body: some View {
    Grid(alignment: .top, horizontalSpacing: 12, verticalSpacing: 10) {
      Text("Settings")
      Divider().padding(.bottom, 10)
    }
  }
}
