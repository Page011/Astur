--!strict
-- Connection state as an icon: Wi-Fi strength, Ethernet, or off. Set
-- `format` to add text; fields are {ssid} {down} {up} {kind}. Rates are only
-- read when the format uses them. Classes: offline, no-internet, stale.
-- popup = true makes a click open the Wi-Fi list (a popup called "network"):
-- networks in range with their signal, a lock mark on secured ones and the
-- connected one marked; a click connects to a network Windows has a saved
-- profile for, or disconnects from the connected one, and the scan button
-- looks again. While Windows keeps network names from desktop apps (location
-- is off for them) the popup says so and offers the settings page.
local rsb = require("@rsb")
local text = require("./lib/format")
local panel = require("./lib/panel")

local LOCATION = "ms-settings:privacy-location"

local function bars(signal: number): string
	if signal >= 70 then
		return "wifi-3"
	elseif signal >= 45 then
		return "wifi-2"
	elseif signal >= 20 then
		return "wifi-1"
	end
	return "wifi-0"
end

return rsb.widget {
	options = {
		format = "",
		every = "2s",
		popup = false,
	},
	permissions = { read = { "network", "wifi" }, control = { "wifi", "apps" } },
	setup = function(props, ctx)
		local network = rsb.services.network
		network:interval(props.every)

		local function glyph(): string
			if network.stale then
				return "globe"
			end
			local kind = network.kind :: string?
			if kind == nil or not network.connected then
				return "network_off"
			elseif kind == "wifi" then
				local signal = network.signal :: number?
				if signal == nil or signal >= 70 then
					return "wifi_4"
				elseif signal >= 45 then
					return "wifi_3"
				elseif signal >= 20 then
					return "wifi_2"
				end
				return "wifi_1"
			elseif kind == "ethernet" then
				return "ethernet"
			end
			return "globe"
		end

		local label: any = nil
		if props.format ~= "" then
			local wants_ssid = text.uses(props.format, "ssid")
			local wants_down = text.uses(props.format, "down")
			local wants_up = text.uses(props.format, "up")
			local wants_kind = text.uses(props.format, "kind")
			label = rsb.text {
				class = "value",
				text = function(): string
					-- No reading yet, or a stale one.
					if network.stale or network.connected == nil then
						return "--"
					end
					local values: text.Values = {
						ssid = if wants_ssid then (network.ssid :: string?) or "" else "",
						kind = if wants_kind then (network.kind :: string?) or "" else "",
						down = if wants_down then rsb.format.rate(network.down_bps) else "",
						up = if wants_up then rsb.format.rate(network.up_bps) else "",
					}
					return text.apply(props.format, values)
				end,
			}
		end

		-- The Wi-Fi popup. The list is read only while it is open.
		local open = rsb.state(false)
		local function networks(): { any }
			local wifi = rsb.services.wifi
			local rows = rsb.each(function(): { any }
				if wifi.stale then
					return {}
				end
				return (wifi.networks :: { any }?) or {}
			end, function(n: any): any
				local ssid = n.ssid :: string?
				local line: any = {
					class = if n.connected then "row net connected" else "row net",
					focusable = true,
					disabled = (not n.connected) and not n.known,
					tooltip = if n.connected
						then nil
						elseif n.known then "Connect"
						else "NOT SAVED - connect once in Windows settings",
					rsb.icon { svg = bars(n.signal) },
					rsb.text { class = "name ellipsis", text = ssid or "Hidden network" },
				}
				if n.secure then
					table.insert(line, rsb.icon { class = "mark", svg = "lock" })
				end
				if n.connected then
					table.insert(
						line,
						panel.icon_button("wifi-off", "Disconnect", function()
							wifi:disconnect()
						end)
					)
				end
				line.on_click = function()
					if not n.connected and n.known and ssid ~= nil then
						wifi:connect(ssid)
					end
				end
				return rsb.row(line)
			end, function(n: any): string
				return (n.ssid :: string?) or "hidden"
			end)
			local function body(): { any }
				local why = panel.service_note(wifi, "Wi-Fi")
				if why ~= nil then
					return { panel.note(why) }
				end
				if wifi.available ~= true then
					return { panel.note("No Wi-Fi adapter") }
				end
				if wifi.access == "needs_location" then
					return {
						rsb.column {
							class = "off",
							panel.note("LOCATION OFF - Windows hides network names"),
							rsb.row {
								class = "row action",
								focusable = true,
								rsb.text { text = "Open location settings" },
								on_click = function()
									rsb.services.apps:open_uri(LOCATION)
								end,
							},
						},
					}
				end
				if #((wifi.networks :: { any }?) or {}) == 0 then
					return { panel.note("No networks in range") }
				end
				return rows()
			end
			return {
				rsb.column {
					class = "network-body",
					rsb.row {
						class = "card head",
						rsb.icon {
							class = "head-icon",
							svg = function(): string
								return if wifi.available == true then "wifi-3" else "wifi-off"
							end,
						},
						rsb.text { class = "title", text = "Wi-Fi" },
						panel.icon_button("refresh", "Scan for networks", function()
							wifi:scan()
						end),
					},
					rsb.scroll { class = "card list", body },
				},
			}
		end
		local pop: any = nil
		if props.popup then
			pop = rsb.popup {
				open = open,
				name = "network",
				class = "panel network",
				placement = "below",
				align = "center",
				networks,
			}
		end

		return rsb.row {
			-- Nothing to show before the first reading.
			visible = function(): boolean
				return network.stale or (network.connected :: boolean?) ~= nil
			end,
			class = function(): string
				local base = if props.popup then "item network button" else "item network"
				if network.stale then
					return base .. " stale"
				elseif not network.connected then
					return base .. " offline"
				elseif not network.internet then
					return base .. " no-internet"
				end
				return base
			end,
			rsb.icon { name = glyph :: any },
			label,
			pop,
			on_click = if props.popup
				then function()
					open.value = not open.value
				end
				else nil,
		}
	end,
}
