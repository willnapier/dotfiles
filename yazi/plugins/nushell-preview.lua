local M = {}

function M:peek()
	local file = self.file.url
	local child = Command("/opt/homebrew/bin/nu")
		:arg("/Users/williamnapier/.config/yazi/scripts/file-preview.nu")
		:env("FILE_PATH", tostring(file))
		:stdout(Command.PIPED)
		:stderr(Command.INHERIT)
		:spawn()

	if not child then
		return self:fallback()
	end

	local output, err = child:wait()
	if not output then
		return self:fallback()
	end

	ya.preview_widgets(self, { ui.Paragraph(self.area, {
		ui.Line(output.stdout),
	}) })
end

function M:seek()
	-- Nushell previews don't support seeking
end

function M:fallback()
	-- Fall back to the built-in code previewer
	ya.manager_emit("peek", {
		"plugin",
		"code",
		file = self.file,
		skip = self.skip,
		window = self.window,
	})
end

return M