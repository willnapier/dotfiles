-- Move PDFs Directly to ZoteroImport Folder Action
-- This script automatically moves PDF files to the Documents/ZoteroImport folder
-- Attach this to folders like Downloads where you save research papers

on adding folder items to this_folder after receiving added_items
	set zoteroImportPath to POSIX path of (path to home folder) & "Documents/ZoteroImport/"
	
	repeat with i from 1 to number of items in added_items
		set this_item to item i of added_items
		set item_info to info for this_item
		set item_name to name of item_info
		set item_extension to name extension of item_info
		
		-- Check if the file is a PDF
		if {"pdf", "PDF"} contains item_extension then
			try
				-- Get the file name
				set fileName to item_name
				
				-- Create destination path
				set destPath to zoteroImportPath & fileName
				
				-- Check if file already exists and add timestamp if needed
				tell application "System Events"
					if exists file destPath then
						-- Add timestamp to make unique
						set currentDate to do shell script "date +%Y%m%d_%H%M%S"
						set nameWithoutExt to text 1 thru -5 of fileName
						set fileName to nameWithoutExt & "_" & currentDate & ".pdf"
						set destPath to zoteroImportPath & fileName
					end if
				end tell
				
				-- Move the file
				do shell script "mv " & quoted form of POSIX path of this_item & " " & quoted form of destPath
				
				-- Log the action
				do shell script "echo '[" & (current date as TEXT) & "] Moved " & fileName & " to ZoteroImport' >> ~/Library/Logs/ZoteroImport.log"
				
				-- Optional: Display notification
				display notification "Moved " & fileName & " to ZoteroImport folder" with title "Zotero Import" subtitle "PDF processed"
				
			on error errMsg
				-- Log error
				do shell script "echo '[" & (current date as TEXT) & "] Error moving " & item_name & ": " & errMsg & "' >> ~/Library/Logs/ZoteroImport.log"
				display notification "Error moving " & item_name with title "Zotero Import Error" subtitle errMsg
			end try
		end if
	end repeat
end adding folder items to