CREATE TABLE session_docs(session_id TEXT PRIMARY KEY, cwd TEXT, updated_at INTEGER, title TEXT, content TEXT, content_hash TEXT);
INSERT INTO session_docs VALUES
 ('recent', '/home/user/project/../project', 1793534340000, 'Title placeholder', 'Content placeholder?', ''),
 ('old', '/home/user/project', 1793530740, '', 'First line placeholder
Tail placeholder', ''),
 ('relative', 'relative/project', 1793534340, 'Skipped', '', '');
