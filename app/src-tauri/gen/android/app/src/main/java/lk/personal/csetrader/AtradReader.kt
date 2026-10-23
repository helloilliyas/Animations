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
 * - **Place in ATrad**: signs in the same way, opens the Buy Sell tab,
 *   picks the exact symbol, sets the Buy/Sell switch (only when it can be
 *   told apart from the submit button) and fills quantity and price. It
 *   never taps the submit button or any confirm and never enters a PIN:
 *   the user checks and confirms the order in ATrad. Any step that cannot
 *   be confirmed stops the filling.
 *
 * Screens and fields are recognised by their visible words.
 */
class AtradReader : AccessibilityService() {
    companion object {
        const val PKG = "com.ironone.atrad"
        private const val TIMEOUT_MS = 120_000L
        /** Readings kept: scrolled pages of the three portfolio tabs. */
        private const val MAX_PAGES = 15
        private val SYMBOL = Regex("^[A-Z0-9&]{1,8}\\.[A-Z]\\d{4}$")

        @Volatile
        private var active = false
        private var startedAt = 0L
        private var signInTapped = false
        private var detailedTapped = false
        /** Portfolio tabs still to read after Detailed. */
        private val tabsToRead = mutableListOf<String>()
        private var visitingTab = false
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
            tabsToRead.clear()
            visitingTab = false
            tabsToRead.addAll(listOf("summary", "valuation"))
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
        private var selectAt = 0L
        private var typed = false
        private var sideTaps = 0

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
                sideTaps = 0
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
        // 3. The holdings: keep what is on screen, scroll for more rows,
        //    then read the Summary and Valuation tabs too (the average cost
        //    is in one of them; Detailed has quantities).
        // A tab opened for reading that has no holdings table (a chart):
        // keep its text and move on.
        if (visitingTab && texts.none { isHoldingsLabel(it) }) {
            visitingTab = false
            if (pages.lastOrNull() != texts) pages.add(texts)
            nextTabOrFinish(now)
            return
        }
        if (texts.any { isHoldingsLabel(it) }) {
            visitingTab = false
            if (pages.lastOrNull() != texts) pages.add(texts)
            val before = texts
            if (pages.size < MAX_PAGES && scrollForward(root)) {
                lastTap = now
                // Nothing new after scrolling: move on to the next tab.
                handler.postDelayed({
                    val after = rootInActiveWindow?.let { collect(it) }
                    if (active && after == before) nextTabOrFinish(now)
                }, 1500)
                return
            }
            nextTabOrFinish(now)
            return
        }
        // 4. Find the way there: the Portfolio tab.
        val target = find(root) { isWord(it, "portfolio") }
        if (target != null && tap(target)) lastTap = now
    }

    /**
     * Fills ATrad's Buy Sell screen in checked steps, never tapping the
     * submit button (it shares its word with the Buy/Sell switch):
     *
     * 0. Open Buy Sell. If the stock box (a tap-to-pick selector showing the
     *    current symbol) is not the order's stock, tap it.
     * 1. In the selector, type the code and tap the result that is exactly
     *    the order's symbol.
     * 2. Back on Buy Sell, check the stock box shows that symbol, set the
     *    Buy/Sell switch, then fill quantity and price.
     * 3. Check ATrad took them: the boxes show them and Order Value equals
     *    quantity x price. Otherwise warn loudly.
     *
     * Any step that cannot be confirmed stops the filling.
     */
    private fun orderStep(root: AccessibilityNodeInfo, texts: List<String>, now: Long, o: Order) {
        val onOrderScreen = texts.any { isWord(it, "buying power", "order value") }
        val shown = findSymbol(root) { true }
        when (orderStage) {
            0 -> {
                if (!onOrderScreen) {
                    if (tabTappedAt == 0L) {
                        find(root) { isWord(it, "buy sell", "buy/sell", "buysell", "buy & sell") }?.let {
                            if (tap(it)) {
                                tabTappedAt = now
                                lastTap = now
                            }
                        }
                    } else if (now - tabTappedAt > 15_000) {
                        finishOrder("Could not open Buy Sell. The order is copied — enter it yourself.", texts)
                    }
                    return
                }
                if (shown?.first == o.symbol) {
                    orderStage = 2
                } else if (shown != null && tap(shown.second)) {
                    orderStage = 1
                    typed = false
                    selectAt = now
                    lastTap = now
                    return
                } else {
                    finishOrder("Could not open the stock selector — choose ${o.symbol} yourself. Nothing was filled.", texts)
                    return
                }
            }
            1 -> {
                if (onOrderScreen) {
                    if (shown?.first == o.symbol) {
                        orderStage = 2
                    } else if (now - selectAt > 4_000) {
                        finishOrder("Could not select ${o.symbol} — choose it yourself. Nothing was filled.", texts)
                        return
                    } else {
                        return
                    }
                } else {
                    if (now - selectAt > 20_000) {
                        finishOrder("Could not find ${o.symbol} in ATrad's search — choose it yourself. Nothing was filled.", texts)
                        return
                    }
                    if (!typed) {
                        val box = firstEditable(root) ?: return
                        setText(box, o.ticker)
                        typed = true
                        lastTap = now
                        return
                    }
                    // Only the exact symbol, never a similar one.
                    find(root) { it.trim().equals(o.symbol, true) }?.takeIf { !it.isEditable }?.let {
                        if (tap(it)) lastTap = now
                    }
                    return
                }
            }
        }
        if (orderStage == 2) {
            if (shown?.first != o.symbol) {
                finishOrder("ATrad shows ${shown?.first ?: "another stock"}, not ${o.symbol}. Nothing was filled.", texts)
                return
            }
            // Buy or Sell: the switch and the submit button share the word,
            // and the submit button follows the switch. The order's side
            // twice means it is selected; once (the other twice) means the
            // only node with that word is the switch, so it is safe to tap.
            val want = if (o.side.equals("sell", true)) "sell" else "buy"
            val other = if (want == "sell") "buy" else "sell"
            val wantCount = texts.count { isWord(it, want) }
            val otherCount = texts.count { isWord(it, other) }
            if (!(wantCount == 2 && otherCount == 1)) {
                if (wantCount == 1 && otherCount == 2 && sideTaps < 2) {
                    find(root) { isWord(it, want) }?.let {
                        if (tap(it)) {
                            sideTaps++
                            lastTap = now
                            return
                        }
                    }
                }
                finishOrder("Selected ${o.symbol}, but could not switch ATrad to ${want.uppercase()} — choose it yourself. Nothing else was filled.", texts)
                return
            }
            val qtyField = fieldFor(root, listOf("quantity", "qty", "order qty", "order quantity"))
            val priceField = fieldFor(root, listOf("price", "limit price", "order price"))
            if (qtyField == null || priceField == null || qtyField == priceField) {
                finishOrder("Selected ${o.symbol}, but could not tell the quantity and price boxes apart — enter them yourself.", texts)
                return
            }
            setText(qtyField, o.qty)
            setText(priceField, o.price)
            orderStage = 3
            lastTap = now
            return
        }
        // 3. Verify what ATrad shows.
        val qtyShown = fieldFor(root, listOf("quantity", "qty", "order qty", "order quantity"))?.text?.toString()
        val priceShown = fieldFor(root, listOf("price", "limit price", "order price"))?.text?.toString()
        val value = texts.indexOfFirst { isWord(it, "order value") }.takeIf { it >= 0 }
            ?.let { texts.getOrNull(it + 1) }?.let { num(it) }
        val q = num(o.qty)
        val p = num(o.price)
        val want = if (o.side.equals("sell", true)) "sell" else "buy"
        val other = if (want == "sell") "buy" else "sell"
        val sideOk = texts.count { isWord(it, want) } == 2 && texts.count { isWord(it, other) } == 1
        val ok = shown?.first == o.symbol && sideOk && num(qtyShown) == q && num(priceShown) == p &&
            value != null && q != null && p != null && kotlin.math.abs(value - q * p) < 0.5
        if (ok) {
            finishOrder("Filled ✓ ${want.uppercase()} ${o.symbol} ${o.qty} @ ${o.price} (order value ${"%,.2f".format(value)}). Check it, then confirm in ATrad.", texts)
        } else {
            finishOrder("⚠ ATrad did not take the order correctly (stock ${shown?.first}, side ${if (sideOk) want.uppercase() else "?"}, quantity ${qtyShown}, price ${priceShown}, order value ${value}). Check every field before confirming.", texts)
        }
    }

    private fun num(s: String?): Double? = s?.replace(",", "")?.trim()?.toDoubleOrNull()

    private fun firstEditable(root: AccessibilityNodeInfo): AccessibilityNodeInfo? {
        fun walk(n: AccessibilityNodeInfo?): AccessibilityNodeInfo? {
            n ?: return null
            if (n.isEditable) return n
            for (i in 0 until n.childCount) walk(n.getChild(i))?.let { return it }
            return null
        }
        return walk(root)
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

    /** Opens the next portfolio tab still to read, or finishes. */
    private fun nextTabOrFinish(now: Long) {
        val root = rootInActiveWindow
        while (tabsToRead.isNotEmpty() && root != null) {
            val tab = tabsToRead.removeAt(0)
            val node = find(root) { isWord(it, tab) } ?: continue
            if (tap(node)) {
                visitingTab = true
                lastTap = now
                return
            }
        }
        finish(true, "")
    }

    /** Column or field names of a holdings list. */
    private fun isHoldingsLabel(t: String): Boolean {
        val l = t.lowercase().trim()
        return (l.contains("holding") && (l.contains("qty") || l.contains("quantity"))) ||
            l.startsWith("avg. cost") || l.startsWith("avg cost") || l == "quantity" ||
            l == "market value"
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
