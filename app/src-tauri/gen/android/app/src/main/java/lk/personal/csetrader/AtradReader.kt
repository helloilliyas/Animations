package lk.personal.csetrader

import android.accessibilityservice.AccessibilityService
import android.content.ClipData
import android.content.ClipboardManager
import android.content.ComponentName
import android.content.Context
import android.content.Intent
import android.os.Handler
import android.os.Looper
import android.provider.Settings
import android.view.accessibility.AccessibilityEvent
import android.view.accessibility.AccessibilityNodeInfo
import org.json.JSONArray
import org.json.JSONObject
import java.io.File

/**
 * Reads holdings from the ATrad app (Portfolio → Detailed) when the user
 * taps "Sync from ATrad". Android only shows this service ATrad's windows
 * (see res/xml/atrad_accessibility.xml).
 *
 * What it does inside ATrad, and nothing more: taps Sign in when the login
 * is already filled in, taps the Portfolio and Detailed tabs, scrolls the
 * holdings list, and reads the text. It never types and never touches
 * order screens. Screens are recognised by their visible words.
 */
class AtradReader : AccessibilityService() {
    companion object {
        const val PKG = "com.ironone.atrad"
        private const val TIMEOUT_MS = 120_000L
        /** Readings kept: one per opened row plus scrolled pages. */
        private const val MAX_PAGES = 80
        private val SYMBOL = Regex("^[A-Z0-9&]{1,8}\\.[A-Z]\\d{4}$")

        @Volatile
        private var active = false
        private var startedAt = 0L
        private var signInTapped = false
        private var detailedTapped = false
        /** Holdings whose row has been opened to show its details. */
        private val opened = mutableSetOf<String>()
        private var lastTap = 0L
        private val pages = mutableListOf<List<String>>()

        fun installed(ctx: Context): Boolean =
            ctx.packageManager.getLaunchIntentForPackage(PKG) != null

        fun enabled(ctx: Context): Boolean {
            val me = ComponentName(ctx, AtradReader::class.java).flattenToString()
            val list = Settings.Secure.getString(
                ctx.contentResolver, Settings.Secure.ENABLED_ACCESSIBILITY_SERVICES
            ) ?: return false
            return list.split(':').any { it.equals(me, ignoreCase = true) }
        }

        fun captureFile(ctx: Context) = File(ctx.filesDir, "atrad-capture.json")

        /** Opens ATrad; the service takes it from there. */
        fun startSync(ctx: Context): Boolean {
            val launch = ctx.packageManager.getLaunchIntentForPackage(PKG) ?: return false
            active = true
            startedAt = System.currentTimeMillis()
            signInTapped = false
            detailedTapped = false
            opened.clear()
            lastTap = 0L
            pages.clear()
            ctx.startActivity(launch.addFlags(Intent.FLAG_ACTIVITY_NEW_TASK))
            return true
        }

        /** Copies an order to the clipboard and opens ATrad (no automation). */
        fun openWithOrder(ctx: Context, text: String): Boolean {
            val launch = ctx.packageManager.getLaunchIntentForPackage(PKG) ?: return false
            val cm = ctx.getSystemService(ClipboardManager::class.java)
            cm.setPrimaryClip(ClipData.newPlainText("ATrad order", text))
            ctx.startActivity(launch.addFlags(Intent.FLAG_ACTIVITY_NEW_TASK))
            return true
        }
    }

    private val handler = Handler(Looper.getMainLooper())
    private var pending = false
    private var retryScheduled = false

    override fun onServiceConnected() {}

    override fun onInterrupt() {}

    override fun onAccessibilityEvent(event: AccessibilityEvent?) {
        if (!active || pending) return
        // Let the screen settle before looking at it.
        pending = true
        handler.postDelayed({
            pending = false
            try {
                step()
            } catch (e: Exception) {
                finish(false, "Reading ATrad failed: ${e.message}")
            }
        }, 700)
    }

    private fun step() {
        if (!active) return
        if (System.currentTimeMillis() - startedAt > TIMEOUT_MS) {
            finish(false, "Timed out: open Portfolio → Detailed in ATrad and try again")
            return
        }
        val root = rootInActiveWindow ?: return
        if (root.packageName?.toString() != PKG) return
        val texts = collect(root)
        val now = System.currentTimeMillis()
        if (now - lastTap < 1500) {
            // Look again once the last tap has had time to show.
            if (!retryScheduled) {
                retryScheduled = true
                handler.postDelayed({
                    retryScheduled = false
                    try {
                        step()
                    } catch (e: Exception) {
                        finish(false, "Reading ATrad failed: ${e.message}")
                    }
                }, 1600 - (now - lastTap))
            }
            return
        }

        // 1. Login screen with the details already filled in: tap Sign in once.
        if (!signInTapped && hasFilledField(root)) {
            find(root) { isWord(it, "sign in", "login", "log in") }?.let {
                signInTapped = tap(it)
                lastTap = now
                return
            }
        }
        // 2. On the portfolio, open the Detailed tab (it has the average
        //    cost) before reading.
        if (!detailedTapped) {
            find(root) { isWord(it, "detailed") }?.let {
                if (tap(it)) {
                    detailedTapped = true
                    lastTap = now
                    return
                }
            }
        }
        // 3. The holdings: keep what is on screen (with any opened row's
        //    details), open the next row (its ˅ shows the average cost),
        //    then scroll for more rows.
        if (texts.any { isHoldingsLabel(it) }) {
            if (pages.lastOrNull() != texts) pages.add(texts)
            if (pages.size >= MAX_PAGES) {
                finish(true, "")
                return
            }
            val next = findSymbol(root) { it !in opened }
            if (next != null) {
                opened.add(next.first)
                if (tap(next.second)) {
                    lastTap = now
                    return
                }
            }
            val before = texts
            if (!scrollForward(root)) {
                finish(true, "")
                return
            }
            lastTap = now
            // Nothing new after scrolling: the end of the list.
            handler.postDelayed({
                val after = rootInActiveWindow?.let { collect(it) }
                if (active && after == before && findSymbol(rootInActiveWindow ?: return@postDelayed) { it !in opened } == null) {
                    finish(true, "")
                }
            }, 1500)
            return
        }
        // A row tap opened another screen: go back and keep what was read.
        if (opened.isNotEmpty()) {
            performGlobalAction(GLOBAL_ACTION_BACK)
            finish(true, "")
            return
        }
        // 4. Find the way there: the Portfolio tab.
        val target = find(root) { isWord(it, "portfolio") }
        if (target != null && tap(target)) lastTap = now
    }

    /** Column or field names of a holdings list. */
    private fun isHoldingsLabel(t: String): Boolean {
        val l = t.lowercase().trim()
        return (l.contains("holding") && (l.contains("qty") || l.contains("quantity"))) ||
            l.startsWith("avg. cost") || l.startsWith("avg cost") || l == "quantity"
    }

    private fun isWord(t: String, vararg words: String) =
        words.any { t.trim().equals(it, ignoreCase = true) }

    private fun textOf(n: AccessibilityNodeInfo): String? =
        (n.text ?: n.contentDescription)?.toString()?.trim()?.takeIf { it.isNotEmpty() }

    /** All text in reading order. */
    private fun collect(root: AccessibilityNodeInfo): List<String> {
        val out = mutableListOf<String>()
        fun walk(n: AccessibilityNodeInfo?) {
            n ?: return
            if (!n.isPassword) textOf(n)?.let { out.add(it) }
            for (i in 0 until n.childCount) walk(n.getChild(i))
        }
        walk(root)
        return out
    }

    /** The first holding row (by its symbol text) that `want` accepts. */
    private fun findSymbol(
        root: AccessibilityNodeInfo,
        want: (String) -> Boolean,
    ): Pair<String, AccessibilityNodeInfo>? {
        fun walk(n: AccessibilityNodeInfo?): Pair<String, AccessibilityNodeInfo>? {
            n ?: return null
            n.text?.toString()?.trim()?.let { if (SYMBOL.matches(it) && want(it)) return it to n }
            for (i in 0 until n.childCount) walk(n.getChild(i))?.let { return it }
            return null
        }
        return walk(root)
    }

    private fun find(root: AccessibilityNodeInfo, match: (String) -> Boolean): AccessibilityNodeInfo? {
        fun walk(n: AccessibilityNodeInfo?): AccessibilityNodeInfo? {
            n ?: return null
            textOf(n)?.let { if (match(it)) return n }
            for (i in 0 until n.childCount) walk(n.getChild(i))?.let { return it }
            return null
        }
        return walk(root)
    }

    /** A text field with something already in it (the saved username). */
    private fun hasFilledField(root: AccessibilityNodeInfo): Boolean {
        fun walk(n: AccessibilityNodeInfo?): Boolean {
            n ?: return false
            if (n.isEditable && !n.isPassword && !n.text.isNullOrBlank()) return true
            for (i in 0 until n.childCount) if (walk(n.getChild(i))) return true
            return false
        }
        return walk(root)
    }

    /** Taps the node or its nearest tappable parent. */
    private fun tap(node: AccessibilityNodeInfo): Boolean {
        var n: AccessibilityNodeInfo? = node
        while (n != null && !n.isClickable) n = n.parent
        return n?.performAction(AccessibilityNodeInfo.ACTION_CLICK) == true
    }

    private fun scrollForward(root: AccessibilityNodeInfo): Boolean {
        fun walk(n: AccessibilityNodeInfo?): AccessibilityNodeInfo? {
            n ?: return null
            if (n.isScrollable) return n
            for (i in 0 until n.childCount) walk(n.getChild(i))?.let { return it }
            return null
        }
        return walk(root)?.performAction(AccessibilityNodeInfo.ACTION_SCROLL_FORWARD) == true
    }

    /** Saves what was read and returns to CSE Trader. */
    private fun finish(ok: Boolean, message: String) {
        if (!active) return
        active = false
        val texts = JSONArray()
        pages.forEach { page -> page.forEach { texts.put(it) } }
        val out = JSONObject()
            .put("ok", ok && pages.isNotEmpty())
            .put("message", if (ok && pages.isEmpty()) "No holdings found on the screen" else message)
            .put("at", System.currentTimeMillis())
            .put("pages", pages.size)
            .put("texts", texts)
        captureFile(this).writeText(out.toString())
        startActivity(
            Intent(this, MainActivity::class.java)
                .addFlags(Intent.FLAG_ACTIVITY_NEW_TASK or Intent.FLAG_ACTIVITY_REORDER_TO_FRONT)
        )
    }
}
