--!strict
-- The current media session: a play or pause icon (what the player is
-- doing now) and the track. A click plays or pauses, a right click skips to
-- the next track. `hide` is "idle" (hidden while there is no session) or
-- "never". Format fields: {artist} {title} {album}. Classes: playing,
-- paused, idle, stale.
--   progress  true draws a thin line along the bottom of the widget, as long
--             as the played part of the track (needs a player that reports
--             its position; it moves once a second while playing)
--   icon      "glyph" (the default) shows the play or pause glyph from the
--             icon font; "app" shows the player's own icon first (the
--             `music` icon when Windows has none for it) and draws the glyph
--             of the click: pause while a track plays, play while it is
--             paused (bundled SVG icons)
--   popup     true makes a left click open the player (a popup called
--             "media": cover, title, artist, a seek slider, previous, play
--             or pause, next) instead of playing or pausing; a right click
--             still skips to the next track
local rsb = require("@rsb")
local text = require("./lib/format")
local panel = require("./lib/panel")

return rsb.widget {
	about = { title = "Media", opens = { "media" } },
	options = {
		format = "{artist} - {title}",
		max_width = 280,
		hide = "idle",
		progress = false,
		icon = "glyph",
		popup = false,
	},
	permissions = { control = { "media" }, read = { "media" } },
	setup = function(props, ctx)
		local media = rsb.services.media
		local app = props.icon == "app"

		local function glyph(): string
			if media.stale or media.available ~= true then
				return "music"
			end
			-- The glyph shows what a click does when the player's icon leads:
			-- pause while a track plays.
			return if (media.playing == true) ~= app then "play" else "pause"
		end

		-- Played part of the track, 0 to 1; nil when the player gives no
		-- length or position. A playing track is aged by the time since the
		-- player's reading; the tick below makes this run again each second.
		local tick = rsb.state(0)
		if props.progress then
			rsb.effect(function()
				if media.playing == true and not media.stale then
					rsb.timer.every(1, function()
						tick.value += 1
					end)
				end
			end)
		end
		local function fraction(): number?
			local _ = tick.value
			local length = media.length :: number?
			local position = media.position :: number?
			if media.stale or length == nil or position == nil or length <= 0 then
				return nil
			end
			local at = media.position_at :: number?
			if media.playing == true and at ~= nil then
				position += math.max(0, panel.epoch_ms(rsb.time.now("utc")) - at) / 1000
			end
			return math.clamp(position / length, 0, 1)
		end

		local function image_key(): string?
			if media.stale then
				return nil
			end
			return media.app_icon :: string?
		end

		-- Leading icons: the glyph from the icon font, or (icon = "app") the
		-- player's own icon, the music note until Windows gives one, and the
		-- play or pause glyph.
		local function leading(): { any }
			local out: { any } = {}
			local key = image_key()
			if key ~= nil then
				table.insert(out, rsb.image { class = "app-icon", src = key })
			else
				table.insert(out, rsb.icon { class = "app-icon", svg = "music" })
			end
			table.insert(out, rsb.icon { class = "state", svg = glyph })
			return out
		end
		local first: any = if app then leading else rsb.icon { name = glyph :: any }

		local label = rsb.text {
			class = "value ellipsis",
			style = { max_width = props.max_width },
			text = function(): string
				if media.stale then
					return "--"
				elseif media.available ~= true then
					return ""
				end
				return text.tidy(text.apply(props.format, {
					artist = media.artist,
					title = media.title,
					album = media.album,
				}))
			end,
		}

		local function state_class(): string
			if media.stale then
				return "item media button stale"
			elseif media.available ~= true then
				return "item media button idle"
			end
			return if media.playing then "item media button playing" else "item media button paused"
		end

		-- A paused track still shows, so it can be resumed.
		local function shown(): boolean
			if media.stale then
				return true
			end
			return media.available == true or props.hide == "never"
		end

		local function play_pause()
			media:play_pause()
		end

		local function next_track()
			media:next()
		end

		-- The player popup: cover, title, artist, a seek slider and the three
		-- transport buttons. A timer moves the slider while a track plays, and
		-- only while the popup is open.
		local open = rsb.state(false)
		local function transport(icon: any, tip: string, run: () -> (), off: () -> boolean, first: boolean?): any
			return rsb.row {
				class = "icon-btn transport",
				focusable = true,
				autofocus = first,
				tooltip = tip,
				disabled = off,
				rsb.icon { svg = icon },
				on_click = run,
			}
		end
		local function player(): { any }
			local tick = rsb.state(0)
			rsb.effect(function()
				if media.playing == true and not media.stale then
					rsb.timer.every(1, function()
						tick.value += 1
					end)
				end
			end)
			local function played(): number
				local _ = tick.value
				local length = media.length :: number?
				local position = panel.media_position(media)
				if length == nil or position == nil or length <= 0 then
					return 0
				end
				return math.clamp(position / length, 0, 1)
			end
			local function say(value: string?, none: string): string
				if media.stale then
					return "--"
				elseif media.available ~= true then
					return none
				end
				return value or ""
			end
			return {
				rsb.row {
					class = "media-pop",
					rsb.box {
						class = "art",
						function(): { any }
							local key = if media.stale then nil else (media.art :: string?)
							if key ~= nil then
								return { rsb.image { src = key, width = 140, height = 140 } }
							end
							return { rsb.icon { svg = "music" } }
						end,
					},
					rsb.column {
						class = "info",
						rsb.column {
							class = "lines",
							rsb.text {
								class = "title ellipsis",
								text = function(): string
									return say(media.title :: string?, "Nothing is playing")
								end,
							},
							rsb.text {
								class = "artist ellipsis",
								text = function(): string
									return say(media.artist :: string?, "")
								end,
							},
						},
						rsb.column {
							class = "controls",
						rsb.slider {
							class = "seek",
							value = played,
							min = 0,
							max = 1,
							step = 0.001,
							disabled = function(): boolean
								return media.can_seek ~= true
							end,
							on_change = function(value: number, done: boolean)
								local length = media.length :: number?
								if done and length ~= nil then
									media:seek(value * length)
								end
							end,
						},
						rsb.row {
							class = "transport-row",
							transport("previous", "Previous", function()
								media:previous()
							end, function(): boolean
								return media.can_previous ~= true
							end),
							transport(function(): string
								return if media.playing == true then "pause" else "play"
							end, "Play or pause", play_pause, function(): boolean
								return media.can_play_pause ~= true
							end, true),
							transport("next", "Next", next_track, function(): boolean
								return media.can_next ~= true
							end),
						},
						},
					},
				},
			}
		end
		local pop: any = nil
		local click = play_pause
		if props.popup then
			click = function()
				open.value = not open.value
			end
			pop = rsb.popup {
				open = open,
				name = "media",
				class = "panel media",
				placement = "below",
				align = "center",
				player,
			}
		end

		if not props.progress then
			return rsb.row {
				visible = shown,
				class = state_class,
				first,
				label,
				pop,
				on_click = click,
				on_right_click = next_track,
			}
		end

		-- With the progress line the widget is a column: the content row, and
		-- under it the line (the row's bottom margin lets the line sit on the
		-- widget's own bottom edge without moving the content).
		return rsb.column {
			visible = shown,
			class = function(): string
				return state_class() .. " with-progress"
			end,
			rsb.row { class = "media-main", first, label },
			rsb.box {
				class = "progress",
				visible = function(): boolean
					return fraction() ~= nil
				end,
				style = function(): { [string]: any }
					return { width = string.format("%.1f%%", (fraction() or 0) * 100) }
				end,
			},
			pop,
			on_click = click,
			on_right_click = next_track,
		}
	end,
}
