--!strict
-- The power menu: four chevron buttons over a dimmed monitor (power off,
-- restart, sleep, lock). It is a popup called "power" that covers the monitor,
-- so it opens only from a click or a key: the dashboard's power tile opens it,
-- and so does `rsb popup power`. There is no confirmation step, so Lock has
-- the focus when it opens and an action fires only on a deliberate press.
-- Lock fades the menu out to a dark curtain and then asks Windows to lock the
-- PC; RSB never draws a lock screen of its own. If Windows refuses an action
-- the reason shows under the buttons until the next reading.
-- The widget itself is an invisible anchor: list it in a zone of bar.toml.
-- Class: power-anchor (the anchor); power, locking (the popup).
local rsb = require("@rsb")

return rsb.widget {
	about = { title = "Power menu", visible = false, opens = { "power" } },
	permissions = { control = { "power", "session" }, overlay = true },
	setup = function(props, ctx)
		local power = rsb.services.power
		local shown = rsb.state(false)
		local locking = rsb.state(false)

		local function button(n: number, icon: string, label: string, run: () -> (), more: any?): any
			local def: any = {
				class = "power-btn b" .. n,
				focusable = true,
				tooltip = label,
				rsb.box {
					class = "tile",
					rsb.icon { svg = icon },
				},
				on_click = run,
			}
			if more ~= nil then
				for k, v in more :: { [string]: any } do
					def[k] = v
				end
			end
			return rsb.row(def)
		end

		local function lock()
			if locking.value then
				return
			end
			locking.value = true
			rsb.task.spawn(function()
				rsb.task.sleep("150ms")
				power:lock()
			end)
		end

		return rsb.box {
			class = "power-anchor",
			rsb.popup {
				open = shown,
				name = "power",
				class = function(): string
					return if locking.value then "power locking" else "power"
				end,
				placement = "screen",
				on_close = function(reason: string)
					locking.value = false
				end,
				function(): { any }
					return {
						rsb.column {
							class = "power-menu",
							rsb.row {
								class = "power-row",
								nav = "horizontal",
								button(1, "power", "Power off", function()
									power:shut_down()
								end),
								button(2, "restart", "Restart", function()
									power:restart()
								end),
								button(3, "sleep", "Sleep", function()
									-- Sleep leaves the menu: nothing is left up when the PC wakes.
									power:sleep()
									shown.value = false
								end, {
									disabled = function(): boolean
										return power.can_sleep == false
									end,
								}),
								button(4, "lock", "Lock", lock, { autofocus = true }),
							},
							rsb.text {
								class = "power-error",
								visible = function(): boolean
									return power.status == "error"
								end,
								text = function(): string
									return (power.error :: string?) or "FAILED - Windows refused"
								end,
							},
						},
					}
				end,
			},
		}
	end,
}
