# contact-sheet

Puts thumbnails of many pictures on one sheet, in the chosen order (the file order list), several thumbnails across (3 to 8). Each picture gives one thumbnail (the first frame of an animation, turned upright from its EXIF orientation); each PDF gives one thumbnail per page. Names are written under the thumbnails: the file's name, or "name p. 3" for a PDF page. Captions can be turned off.

Sheets are A4 (1240 × 1754 px), US Letter (1275 × 1650 px) or screen-sized (1920 px wide, up to 10 rows tall). When the thumbnails don't fit one sheet, more are made: `contact-sheet-1.png`, `contact-sheet-2.png`, and so on (just `contact-sheet.png` when there is one). Saved as a PDF there is one file, `contact-sheet.pdf`, with a page for each sheet.

At most 500 pages from one PDF and 2,000 thumbnails in all, checked before the job starts. Password-protected PDFs ask for the password. Thumbnails are made and put on a sheet one sheet at a time, so memory doesn't grow with the number of files.

Engines: Pillow, pillow-heif (HEIC), pypdfium2 (PDF pages), pikepdf (PDF checks), ReportLab (PDF output).
