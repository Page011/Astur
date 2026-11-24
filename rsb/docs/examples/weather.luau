--!strict
-- Current temperature from open-meteo.com (public, no key).
--
-- Save as widgets\my\weather.luau, then in bar.toml:
--
--   [[bar]]
--   right = ["my/weather", "clock"]
--
--   [widget."my/weather"]
--   latitude = -30.51
--   longitude = 151.67
--
-- The coordinates choose the place (these are Armidale). A click opens the
-- forecast page in the default browser.
local rsb = require("@rsb")

return rsb.widget {
	options = {
		latitude = -30.51,
		longitude = 151.67,
		every = "15m",
	},
	permissions = { net = { "api.open-meteo.com", "open-meteo.com" } },
	setup = function(props, ctx)
		local url = `https://api.open-meteo.com/v1/forecast?latitude={props.latitude}&longitude={props.longitude}&current=temperature_2m`

		-- Runs now and every `every`, inside a task, so it may wait on the
		-- network. The last good value stays when a request fails.
		local temperature = rsb.poll(props.every, function(): number?
			local response = rsb.http.get(url, { cache = "5m" })
			if not response.ok then
				error(`weather: HTTP {response.status} {response.error or ""}`)
			end
			local data = response:json()
			return data.current.temperature_2m
		end)

		return rsb.row {
			class = function(): string
				return if temperature.error ~= nil then "item weather stale" else "item weather button"
			end,
			rsb.icon { name = "cloud" },
			rsb.text {
				class = "value",
				text = function(): string
					local t = temperature.value
					if t == nil then
						return "--"
					end
					return `{math.round(t)}°`
				end,
			},
			on_click = function()
				rsb.open_url("https://open-meteo.com")
			end,
		}
	end,
}
