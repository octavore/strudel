import Foundation

// Safari launches this process for native message handling.
// This extension uses only content scripts, so the run loop keeps the process
// alive in case Safari does invoke it.
RunLoop.main.run()
