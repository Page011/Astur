--!strict
-- Connection state as an icon: Wi-Fi strength, Ethernet, or off. Set
-- `format` to add text; fields are {ssid} {down} {up} {kind}. Rates are only
-- read when the format uses them. Classes: offline, no-internet, stale.
local rsb = require("@rsb")
local text = require("./lib/format")

return rsb.widget {
	options = {
		format = "",
		every = "2s",
	},
	permissions = { read = { "network" } },
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

		return rsb.row {
			-- Nothing to show before the first reading.
			visible = function(): boolean
				return network.stale or (network.connected :: boolean?) ~= nil
			end,
			class = function(): string
				if network.stale then
					return "item network stale"
				elseif not network.connected then
					return "item network offline"
				elseif not network.internet then
					return "item network no-internet"
				end
				return "item network"
			end,
			rsb.icon { name = glyph :: any },
			label,
		}
	end,
}
