import Tauri
import UIKit

struct ShareArgs: Decodable {
  let text: String
}

/// Opens iOS's share sheet with a text (#66).
class ShareSheetPlugin: Plugin {
  @objc public func share(_ invoke: Invoke) throws {
    let args = try invoke.parseArgs(ShareArgs.self)
    DispatchQueue.main.async {
      guard let presenter = self.manager.viewController else {
        invoke.reject("There is no view to show the share sheet on.")
        return
      }
      let sheet = UIActivityViewController(activityItems: [args.text], applicationActivities: nil)
      // On an iPad the sheet is a popover, which needs a place to point at.
      UIUtils.centerPopover(rootViewController: presenter, popoverController: sheet)
      sheet.popoverPresentationController?.permittedArrowDirections = []
      presenter.present(sheet, animated: true)
      invoke.resolve()
    }
  }
}

@_cdecl("init_plugin_share_sheet")
func initPlugin() -> Plugin {
  return ShareSheetPlugin()
}
