# Material 3 Expressive: a compact checklist

The checklist seimei's web page and app are reviewed against (REVIEW.md). Material 3 Expressive (M3X) is Google's 2025 update to Material 3: a bigger shape scale, spring motion, emphasized type, and new components (button groups, toolbars, loading indicator, wavy progress). Google's own research claims people found key elements up to 4x faster in expressive layouts ([design.google](https://design.google/library/expressive-material-design-google-research)); the launch post is on [blog.google](https://blog.google/products/android/material-3-expressive-android-wearos-launch/).

Sources: m3.material.io renders client-side, so its pages are linked as the canonical guidance, and the numbers below come from the token files Compose ships ([androidx tokens](https://github.com/androidx/androidx/tree/androidx-main/compose/material3/material3/src/commonMain/kotlin/androidx/compose/material3/tokens)) and the [material-components-android docs](https://github.com/material-components/material-components-android/tree/master/docs). Values marked (unconfirmed) were not found in a primary source. Checked 2026-10-24.

## 1. Shape

Guidance: [m3 shape](https://m3.material.io/styles/shape/overview-principles), values: [ShapeTokens.kt](https://github.com/androidx/androidx/blob/androidx-main/compose/material3/material3/src/commonMain/kotlin/androidx/compose/material3/tokens/ShapeTokens.kt).

- [ ] Corners come from one 10-step scale (dp): none 0, extra-small 4, small 8, medium 12, large 16, large-increased 20, extra-large 28, extra-large-increased 32, extra-extra-large 48, full (pill or circle).
- [ ] Bigger and more prominent elements get bigger corners: chips and small buttons 8 to 12, cards 12 to 16, sheets and dialogs 28, the hero element up to 48.
- [ ] Shape contrast carries meaning: mixing round and square in one view draws the eye to the odd one out (a square send button among pills, a squircle avatar among circles).
- [ ] Shape morphing on interaction: buttons go from round to square corners when pressed or selected (pressed corner is one step down: M button full -> 16 square -> 12 pressed). [ButtonMediumTokens.kt](https://github.com/androidx/androidx/blob/androidx-main/compose/material3/material3/src/commonMain/kotlin/androidx/compose/material3/tokens/ButtonMediumTokens.kt)
- [ ] The 35-shape library (Cookie, Clover, Burst, Sunny, Pill, Gem, ...) is for avatars, badges, loading and hero moments, not for every container. [MaterialShapes.kt](https://github.com/androidx/androidx/blob/androidx-main/compose/material3/material3/src/commonMain/kotlin/androidx/compose/material3/MaterialShapes.kt)
- [ ] Grouped lists: items in one group sit 2 to 4 dp apart, the group gets large outer corners and small inner ones (the "segmented" list, see refs/03).

## 2. Motion

Guidance: [m3 motion](https://m3.material.io/styles/motion/overview/how-it-works), values: [ExpressiveMotionTokens.kt](https://github.com/androidx/androidx/blob/androidx-main/compose/material3/material3/src/commonMain/kotlin/androidx/compose/material3/tokens/ExpressiveMotionTokens.kt), [StandardMotionTokens.kt](https://github.com/androidx/androidx/blob/androidx-main/compose/material3/material3/src/commonMain/kotlin/androidx/compose/material3/tokens/StandardMotionTokens.kt), rules: [MDC Motion.md](https://github.com/material-components/material-components-android/blob/master/docs/theming/Motion.md).

- [ ] Motion is springs, not durations: each token is a damping ratio and a stiffness.
- [ ] Spatial springs move, resize or reshape things (position, size, corner). They may overshoot.
- [ ] Effects springs change color and opacity. They never overshoot (damping 1.0).
- [ ] Speed follows size and distance: fast for small parts (switch, button press), default for cards and sheets, slow for full-screen changes.
- [ ] A press that changes shape and color uses the fast spatial spring for the shape and the fast effects spring for the color.
- [ ] Respect reduced motion: keep the state change, drop the bounce.

| Token | Damping | Stiffness | CSS duration | CSS `linear()` easing (simulated, mass 1) |
|---|---|---|---|---|
| expressive spatial fast | 0.6 | 800 | 370 ms | `linear(0, .177, .491, .785, .977, 1.072, 1.093, 1.076, 1.046, 1.019, 1.001, .993, .991, .993, .996, .998, 1)` |
| expressive spatial default | 0.8 | 380 | 410 ms | `linear(0, .103, .3, .506, .684, .817, .907, .962, .994, 1.009, 1.014, 1.014, 1.012, 1.008, 1.006, 1.003, 1)` |
| expressive spatial slow | 0.8 | 200 | 570 ms | same curve as default, longer |
| effects fast | 1.0 | 3800 | 155 ms | `linear(0, .269, .585, .788, .897, .951, .978, .99, .995, .998, 1)` |
| effects default | 1.0 | 1600 | 225 ms | `linear(0, .241, .548, .757, .876, .938, .97, .986, .993, .997, 1)` |
| effects slow | 1.0 | 800 | 300 ms | `linear(0, .218, .518, .732, .858, .927, .964, .982, .991, .996, 1)` |
| standard spatial fast / default / slow | 0.9 | 1400 / 700 / 300 | 180 / 250 / 370 ms | near-critical, no visible overshoot |

The CSS columns are this review's own simulation of the token springs (settled within 0.2%), not published values. CSS `linear()` works in every engine seimei targets (Chrome and Android WebView 113+, Firefox 112+, Safari 17.2+).

## 3. Typography

Guidance: [m3 type scale](https://m3.material.io/styles/typography/type-scale-tokens), values: [TypeScaleTokens.kt](https://github.com/androidx/androidx/blob/androidx-main/compose/material3/material3/src/commonMain/kotlin/androidx/compose/material3/tokens/TypeScaleTokens.kt), [TypographyTokens.kt](https://github.com/androidx/androidx/blob/androidx-main/compose/material3/material3/src/commonMain/kotlin/androidx/compose/material3/tokens/TypographyTokens.kt).

- [ ] Text sizes come from 15 roles (size/line height in sp): display 57/64, 45/52, 36/44; headline 32/40, 28/36, 24/32; title 22/28, 16/24, 14/20; body 16/24, 14/20, 12/16; label 14/20, 12/16, 11/16.
- [ ] Every role has an emphasized twin with the same size and a heavier weight: display, headline, title large and body go 400 -> 500; title medium/small and labels go 500 -> 700 (the token file still carries a TODO, so weights may move).
- [ ] Emphasis marks what to act on or read first (the active state, the key number, the asker's name), not decoration.
- [ ] Variable fonts: Google Sans Flex and Roboto Flex let weight move in small steps, so emphasis can animate (unconfirmed for Google Sans Flex axes beyond weight and optical size).

## 4. Color

Guidance: [m3 color roles](https://m3.material.io/styles/color/roles), values: [ColorSchemeKeyTokens.kt](https://github.com/androidx/androidx/blob/androidx-main/compose/material3/material3/src/commonMain/kotlin/androidx/compose/material3/tokens/ColorSchemeKeyTokens.kt).

- [ ] Roles, not raw colors: primary, secondary, tertiary, each with on-, container, on-container and fixed variants; error and error container; surface, surface-dim, surface-bright, surface-container lowest/low/default/high/highest; inverse surface/on/primary; outline, outline-variant; scrim.
- [ ] Hierarchy comes from tonal surfaces: the page on surface, groups on surface-container, raised or active things on container-high/highest. No shadows needed on a flat dark theme.
- [ ] Containers carry state color: primary-container for the active or vibrant thing, secondary-container for selection (nav indicator), tertiary-container for a contrasting accent, error-container for danger.
- [ ] Component mapping: nav bar container surface-container, active indicator secondary-container. Floating toolbar standard on surface-container, vibrant on primary-container. Contained loading indicator primary-container with on-primary-container. [NavigationBarTokens](https://github.com/androidx/androidx/blob/androidx-main/compose/material3/material3/src/commonMain/kotlin/androidx/compose/material3/tokens/NavigationBarTokens.kt), [FloatingToolbarTokens](https://github.com/androidx/androidx/blob/androidx-main/compose/material3/material3/src/commonMain/kotlin/androidx/compose/material3/tokens/FloatingToolbarTokens.kt), [LoadingIndicatorTokens](https://github.com/androidx/androidx/blob/androidx-main/compose/material3/material3/src/commonMain/kotlin/androidx/compose/material3/tokens/LoadingIndicatorTokens.kt)
- [ ] Disabled: container on-surface at 12%, content at 38%.

## 5. Components

- [ ] Buttons come in five sizes (height dp): XS 32, S 40, M 56, L 96, XL 136. Each has a round (full) and a square shape (XS/S 12, M 16, L/XL 28) and morphs on press. [m3 buttons](https://m3.material.io/components/buttons/overview), [ButtonSmallTokens](https://github.com/androidx/androidx/blob/androidx-main/compose/material3/material3/src/commonMain/kotlin/androidx/compose/material3/tokens/ButtonSmallTokens.kt)
- [ ] Toggle buttons: a button that stays selected switches to the square shape and the filled color. [MDC Button.md](https://github.com/material-components/material-components-android/blob/master/docs/components/Button.md)
- [ ] Button groups: related actions in one row. Standard groups keep gaps and the pressed button widens while its neighbours give way (values unconfirmed); connected groups touch with 2 dp gaps, outer corners full and inner corners small (a modern segmented button). [m3 button groups](https://m3.material.io/components/button-groups/overview), refs/02
- [ ] Split button: a main action plus a menu toggle joined into one pill, for "Allow / Allow with options". [m3 split button](https://m3.material.io/components/split-button/overview)
- [ ] Icon buttons: narrow, default and wide widths, round or square; 48 dp target at least.
- [ ] FAB: S 56 corner 16, M 80, L 96 corner 28; extended FAB S 56, M 96. The FAB menu opens a stack of labelled pills from the FAB. [m3 FAB menu](https://m3.material.io/components/fab-menu/overview), [ExtendedFabSmallTokens](https://github.com/androidx/androidx/blob/androidx-main/compose/material3/material3/src/commonMain/kotlin/androidx/compose/material3/tokens/ExtendedFabSmallTokens.kt)
- [ ] Toolbars replace the bottom app bar: docked (full width, short) or floating (pill, 64 dp, 16 dp off the edge, can pair with a FAB). Standard on surface-container or vibrant on primary-container. The bottom app bar is deprecated. [m3 toolbars](https://m3.material.io/components/toolbars/overview), [MDC BottomAppBar.md](https://github.com/material-components/material-components-android/blob/master/docs/components/BottomAppBar.md), refs/04
- [ ] Loading indicator: a 38 dp shape that morphs through the shape library inside a 48 dp box, plain (primary) or contained (on primary-container). For short waits of unknown length (about 5 s at most: unconfirmed), where a spinner used to go. It replaces indeterminate circular progress for short waits. [m3 loading indicator](https://m3.material.io/components/loading-indicator/overview), refs/05
- [ ] Progress indicators: linear and circular, flat or wavy. Wavy marks an active, ongoing process (amplitude 3, wavelength 40 linear); flat is for static amounts. The track has a 4 dp gap to the indicator and a 4 dp stop dot at its end. [m3 progress](https://m3.material.io/components/progress-indicators/overview), [LinearProgressIndicatorTokens](https://github.com/androidx/androidx/blob/androidx-main/compose/material3/material3/src/commonMain/kotlin/androidx/compose/material3/tokens/LinearProgressIndicatorTokens.kt), refs/06
- [ ] Navigation bar: 64 dp tall (shorter than M3's 80), pill active indicator, label medium. On medium windows the items go horizontal (icon beside label). [m3 navigation bar](https://m3.material.io/components/navigation-bar/overview), refs/07
- [ ] Navigation rail: collapsed 96 dp (narrow 80) or expanded with labels; it replaces the navigation drawer, which is deprecated. A FAB or menu button can sit on top. [m3 navigation rail](https://m3.material.io/components/navigation-rail/overview), [MDC NavigationDrawer.md](https://github.com/material-components/material-components-android/blob/master/docs/components/NavigationDrawer.md)
- [ ] App bars: a search app bar and a flexible top app bar that shrinks on scroll; titles may use the emphasized styles (values unconfirmed).
- [ ] Lists: segmented groups (see Shape), swipe-to-reveal actions, leading avatars in expressive shapes. [MDC List.md](https://github.com/material-components/material-components-android/blob/master/docs/components/List.md), refs/03

## 6. Density, touch and layout

- [ ] Every tappable thing has a 48x48 dp target, even when the visible part is smaller. [Android accessibility defaults](https://developer.android.com/develop/ui/compose/accessibility/api-defaults)
- [ ] Spacing on a 4 dp grid (observed in all token files).
- [ ] Window classes by width: compact < 600, medium 600 to 839, expanded 840 to 1199, large 1200 to 1599, extra-large 1600+. Nav bar on compact, rail from medium up. [window size classes](https://developer.android.com/develop/ui/compose/layouts/adaptive/use-window-size-classes)
- [ ] Primary actions sit where the thumb is: bottom half of a phone, the main action at the end of the reading order.

## 7. When to be expressive

- [ ] Expressive for key moments and the main action (the send button, the decision on a card, the thing that is working), standard for utilitarian lists and settings (Google's own "expressive for most situations, standard for utilitarian products": unconfirmed wording, seen in search excerpts only).
- [ ] One hero per screen: emphasis everywhere is no emphasis.
- [ ] Never at the cost of contrast, target size or reduced motion.

## Reference apps

Images with sources in [refs/](refs/README.md): Google Clock (timers and stopwatch: shape-contrast action buttons, stacked full-width buttons), Google Home (tonal tiles, grouped list rows with trailing round action), Gemini (chat with a single-pill composer, no nav bar on the chat), Google's compose study (standard vs expressive), and the official component images for button groups, segmented lists, toolbars, the loading indicator, wavy progress, nav bar and rail.
