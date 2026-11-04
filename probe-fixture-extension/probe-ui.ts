// Fixture extension for the RPC probe `extension_ui` scenario.
//
// Loaded only for a probe run via:  pi --mode rpc -e probe-fixture-extension/probe-ui.ts
// It registers /probe-ui, which exercises ctx.ui.select + confirm + notify so
// their extension_ui_request events appear on the RPC stdout stream.
//
// This file lives outside ~/.pi/agent/extensions/ and never installs globally.

/** @param {import("@earendil-works/pi-coding-agent").ExtensionAPI} pi */
export default function (pi) {
	pi.registerCommand("probe-ui", {
		description: "Probe: trigger select/confirm/notify UI requests",
		handler: async (_args, ctx) => {
			const choice = await ctx.ui.select("probe select", ["first", "second"]);
			const confirmed = await ctx.ui.confirm("probe confirm", `you picked ${choice ?? "<cancelled>"}`);
			ctx.ui.notify(
				`probe notify: choice=${choice ?? "<cancelled>"} confirmed=${confirmed}`,
				"info",
			);
		},
	});
}
