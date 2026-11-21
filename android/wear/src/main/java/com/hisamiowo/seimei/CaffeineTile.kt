package com.hisamiowo.seimei

import androidx.concurrent.futures.CallbackToFutureAdapter
import androidx.wear.protolayout.ActionBuilders
import androidx.wear.protolayout.ColorBuilders.argb
import androidx.wear.protolayout.DimensionBuilders.dp
import androidx.wear.protolayout.DimensionBuilders.sp
import androidx.wear.protolayout.LayoutElementBuilders
import androidx.wear.protolayout.ModifiersBuilders
import androidx.wear.protolayout.ResourceBuilders
import androidx.wear.protolayout.TimelineBuilders
import androidx.wear.tiles.RequestBuilders
import androidx.wear.tiles.TileBuilders
import androidx.wear.tiles.TileService
import com.google.common.util.concurrent.ListenableFuture

/**
 * The caffeine tile, among the watch's swipe-through tiles: a tap turns caffeine on or off, and it shows the minutes left.
 * Wear OS quick settings show only the system's own toggles, so this is a tile.
 */
class CaffeineTile : TileService() {
    override fun onTileRequest(requestParams: RequestBuilders.TileRequest): ListenableFuture<TileBuilders.Tile> {
        // Tile requests come on the main thread, as Caffeine wants.
        if (requestParams.currentState.lastClickableId == TOGGLE) {
            if (Caffeine.on) Caffeine.stop(this) else Caffeine.start(this, Caffeine.minutes(this))
        }
        val on = Caffeine.on
        val button = LayoutElementBuilders.Box.Builder()
            .setWidth(dp(96f)).setHeight(dp(96f))
            .setModifiers(ModifiersBuilders.Modifiers.Builder()
                .setBackground(ModifiersBuilders.Background.Builder()
                    .setColor(argb(if (on) 0xFFFFB74D.toInt() else 0xFF303030.toInt()))
                    .setCorner(ModifiersBuilders.Corner.Builder().setRadius(dp(48f)).build()).build())
                .setClickable(ModifiersBuilders.Clickable.Builder().setId(TOGGLE)
                    .setOnClick(ActionBuilders.LoadAction.Builder().build()).build())
                .build())
            .addContent(text(if (on) "${Caffeine.minutesLeft()} min" else "off", 20f, if (on) 0xFF000000.toInt() else 0xFFFFFFFF.toInt()))
            .build()
        val layout = LayoutElementBuilders.Column.Builder()
            .addContent(text("caffeine", 16f, 0xFFB0B0B0.toInt()))
            .addContent(LayoutElementBuilders.Spacer.Builder().setHeight(dp(8f)).build())
            .addContent(button)
            .addContent(LayoutElementBuilders.Spacer.Builder().setHeight(dp(8f)).build())
            .addContent(text(if (on) "tap to turn off" else "tap for ${Caffeine.minutes(this)} min", 13f, 0xFFB0B0B0.toInt()))
            .build()
        val tile = TileBuilders.Tile.Builder()
            .setResourcesVersion(RESOURCES)
            // While on, the minutes left count down once a minute.
            .setFreshnessIntervalMillis(if (on) 60_000L else 0L)
            .setTileTimeline(TimelineBuilders.Timeline.fromLayoutElement(layout))
            .build()
        return done(tile)
    }

    @Deprecated("The tiles library still calls it")
    override fun onTileResourcesRequest(requestParams: RequestBuilders.ResourcesRequest): ListenableFuture<ResourceBuilders.Resources> =
        done(ResourceBuilders.Resources.Builder().setVersion(RESOURCES).build())

    private fun text(s: String, size: Float, color: Int) = LayoutElementBuilders.Text.Builder().setText(s)
        .setFontStyle(LayoutElementBuilders.FontStyle.Builder().setSize(sp(size)).setColor(argb(color)).build())
        .build()

    private fun <T : Any> done(value: T): ListenableFuture<T> = CallbackToFutureAdapter.getFuture { it.set(value) }

    private companion object {
        const val TOGGLE = "toggle"
        const val RESOURCES = "1"
    }
}
