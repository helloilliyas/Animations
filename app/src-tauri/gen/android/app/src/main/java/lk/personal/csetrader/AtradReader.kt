package lk.personal.csetrader

import android.accessibilityservice.AccessibilityService
import android.content.ClipData
import android.content.ClipboardManager
import android.content.ComponentName
import android.content.Context
import android.content.Intent
import android.os.Build
import android.os.Bundle
import android.os.Handler
import android.os.Looper
import android.provider.Settings
import android.view.accessibility.AccessibilityEvent
import android.view.accessibility.AccessibilityNodeInfo
import android.widget.Toast
import org.json.JSONArray
import org.json.JSONObject
import java.io.File

/**
 * Works inside the ATrad app only (Android shows this service no other
 * app; see res/xml/atrad_accessibility.xml), and only when the user asks:
 *
 * - **Sync from ATrad**: taps Sign in when the login is already filled in,
 *   opens Portfolio → Detailed, opens each holding's row and reads the
 *   text, then returns to CSE Trader.
 * - **Place in ATrad**: signs in the same way, opens the Buy Sell tab and
 *   fills in the symbol, quantity and price. It never taps Buy, Sell,
 *   Submit or any confirm button and never enters a PIN: the user checks
 *   and confirms the order in ATrad. If the fields cannot be identified
 *   with certainty, nothing is typed.
 *
 * Screens and fields are recognised by their visible words.
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
            order = null
            lastTap = 0L
            pages.clear()
            ctx.startActivity(launch.addFlags(Intent.FLAG_ACTIVITY_NEW_TASK))
            return true
        }

        /** An order to fill in on ATrad's Buy Sell screen. */
        data class Order(val ticker: String, val symbol: String, val side: String, val qty: String, val price: String)

        private var order: Order? = null
        private var orderStage = 0
        private var tabTappedAt = 0L

        fun orderLogFile(ctx: Context) = File(ctx.filesDir, "atrad-order.json")

        /**
         * Copies the order, opens ATrad and (when the reader is on) fills in
         * the Buy Sell screen for the user to confirm.
         */
        fun openWithOrder(ctx: Context, text: String, o: Order?): Boolean {
            val launch = ctx.packageManager.getLaunchIntentForPackage(PKG) ?: return false
            val cm = ctx.getSystemService(ClipboardManager::class.java)
            cm.setPrimaryClip(ClipData.newPlainText("ATrad order", text))
            if (o != null && enabled(ctx)) {
                active = true
                startedAt = System.currentTimeMillis()
                signInTapped = false
                lastTap = 0L
                tabTappedAt = 0L
                orderStage = 0
                order = o
            }
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
            if (order != null) finishOrder("Could not fill the order in time — it is copied; enter it in ATrad.", emptyList())
            else finish(false, "Timed out: open Portfolio → Detailed in ATrad and try again")
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
        // Placing an order: fill in the Buy Sell screen, then stop.
        order?.let {
            orderStep(root, texts, now, it)
            return
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

    /**
     * Fills the Buy Sell screen: symbol first (picking the matching
     * suggestion if a list appears), then quantity and price. Never taps a
     * button on that screen.
     */
    private fun orderStep(root: AccessibilityNodeInfo, texts: List<String>, now: Long, o: Order) {
        val symbolField = fieldFor(root, listOf("symbol", "security", "stock", "search symbol"))
        val qtyField = fieldFor(root, listOf("quantity", "qty", "order qty", "order quantity"))
        val priceField = fieldFor(root, listOf("price", "limit price", "order price"))
        val sure = qtyField != null && priceField != null && qtyField != priceField
        if (!sure) {
            if (tabTappedAt == 0L) {
                find(root) { isWord(it, "buy sell", "buy/sell", "buysell", "buy & sell") }?.let {
                    if (tap(it)) {
                        tabTappedAt = now
                        lastTap = now
                    }
                }
            } else if (now - tabTappedAt > 15_000) {
                finishOrder("Opened Buy Sell. The order is copied — enter it and confirm in ATrad.", texts)
            }
            return
        }
        when (orderStage) {
            0 -> {
                orderStage = 1
                if (symbolField != null && symbolField != qtyField && symbolField != priceField) {
                    setText(symbolField, o.ticker)
                    lastTap = now
                    return
                }
            }
            1 -> {
                orderStage = 2
                // A suggestion list: pick the exact symbol (not the field itself).
                find(root) { it.equals(o.symbol, true) || it.equals(o.ticker, true) || it.startsWith(o.symbol + " ", true) }
                    ?.takeIf { !it.isEditable }
                    ?.let {
                        if (tap(it)) {
                            lastTap = now
                            return
                        }
                    }
            }
        }
        setText(qtyField!!, o.qty)
        setText(priceField!!, o.price)
        finishOrder("Filled: ${o.side.uppercase()} ${o.ticker} ${o.qty} @ ${o.price}. Check Buy/Sell and the details, then confirm in ATrad.", collect(root))
    }

    /** The text box for one of `labels`: by its hint, or just after a label text. */
    private fun fieldFor(root: AccessibilityNodeInfo, labels: List<String>): AccessibilityNodeInfo? {
        val nodes = mutableListOf<AccessibilityNodeInfo>()
        fun walk(n: AccessibilityNodeInfo?) {
            n ?: return
            nodes.add(n)
            for (i in 0 until n.childCount) walk(n.getChild(i))
        }
        walk(root)
        fun norm(s: CharSequence?) = s?.toString()?.trim()?.trimEnd(':', '*', ' ')?.lowercase()
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O) {
            nodes.firstOrNull { it.isEditable && norm(it.hintText) in labels }?.let { return it }
        }
        val i = nodes.indexOfFirst { !it.isEditable && norm(it.text ?: it.contentDescription) in labels }
        if (i < 0) return null
        return nodes.drop(i + 1).take(6).firstOrNull { it.isEditable }
    }

    private fun setText(node: AccessibilityNodeInfo, value: String) {
        val args = Bundle().apply {
            putCharSequence(AccessibilityNodeInfo.ACTION_ARGUMENT_SET_TEXT_CHARSEQUENCE, value)
        }
        node.performAction(AccessibilityNodeInfo.ACTION_FOCUS)
        node.performAction(AccessibilityNodeInfo.ACTION_SET_TEXT, args)
    }

    /** Ends order filling, leaving the user in ATrad to check and confirm. */
    private fun finishOrder(message: String, texts: List<String>) {
        active = false
        order = null
        orderLogFile(this).writeText(
            JSONObject().put("message", message).put("at", System.currentTimeMillis())
                .put("texts", JSONArray(texts)).toString()
        )
        Toast.makeText(this, message, Toast.LENGTH_LONG).show()
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
