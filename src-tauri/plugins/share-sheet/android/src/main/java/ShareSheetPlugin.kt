package chat.peoples.sharesheet

import android.app.Activity
import android.content.Intent
import app.tauri.annotation.Command
import app.tauri.annotation.InvokeArg
import app.tauri.annotation.TauriPlugin
import app.tauri.plugin.Invoke
import app.tauri.plugin.Plugin

@InvokeArg
class ShareArgs {
    lateinit var text: String
}

/** Opens Android's share chooser with a text (#66). */
@TauriPlugin
class ShareSheetPlugin(private val activity: Activity) : Plugin(activity) {
    @Command
    fun share(invoke: Invoke) {
        val args = invoke.parseArgs(ShareArgs::class.java)
        val send = Intent(Intent.ACTION_SEND).apply {
            type = "text/plain"
            putExtra(Intent.EXTRA_TEXT, args.text)
        }
        activity.runOnUiThread {
            activity.startActivity(Intent.createChooser(send, null))
            invoke.resolve()
        }
    }
}
