-- Selected columns of the documented SQLite schema; omitted columns are unused.
CREATE TABLE session(id TEXT PRIMARY KEY, parent_id TEXT, directory TEXT, title TEXT, time_created INTEGER, time_updated INTEGER);
CREATE TABLE message(id TEXT PRIMARY KEY, session_id TEXT, time_created INTEGER, time_updated INTEGER, data TEXT);
CREATE TABLE part(id TEXT PRIMARY KEY, message_id TEXT, session_id TEXT, time_created INTEGER, time_updated INTEGER, data TEXT);
CREATE INDEX message_session ON message(session_id);
CREATE INDEX part_message ON part(message_id);
INSERT INTO session VALUES
 ('working', NULL, '/home/user/project', 'Working placeholder', 1793530000000, 1793534340000),
 ('stuck', NULL, '/home/user/project', 'Stuck placeholder', 1793530000000, 1793532600000),
 ('waiting', NULL, '/home/user/project', 'Waiting placeholder', 1793530000000, 1793534340000),
 ('done', NULL, '/home/user/project', '', 1793530000000, 1793534340000),
 ('idle', NULL, '/home/user/project', 'Idle placeholder', 1793530000000, 1793530800000),
 ('error', NULL, '/home/user/project', 'Error placeholder', 1793530000000, 1793534340000),
 ('relative', NULL, 'relative/project', 'Skipped', 1793530000000, 1793534340000),
 ('child', 'done', '/home/user/project', 'Child skipped', 1793530000000, 1793534340000);
INSERT INTO message SELECT id || '-user', id, time_created, time_created, '{"role":"user"}' FROM session;
INSERT INTO part SELECT id || '-prompt', id || '-user', id, time_created, time_created, '{"type":"text","text":"User placeholder"}' FROM session;
INSERT INTO message SELECT id || '-assistant', id, time_updated, time_updated,
 CASE WHEN id IN ('working', 'stuck') THEN '{"role":"assistant","time":{"created":1793534340000}}'
 WHEN id = 'error' THEN '{"role":"assistant","error":{"name":"APIError"}}'
 ELSE '{"role":"assistant","time":{"completed":1793534340000},"finish":"stop"}' END FROM session;
INSERT INTO part SELECT id || '-answer', id || '-assistant', id, time_updated, time_updated,
 CASE WHEN id = 'waiting' THEN '{"type":"text","text":"Decision placeholder?"}'
 ELSE '{"type":"text","text":"Assistant placeholder"}' END FROM session;
INSERT INTO part SELECT id || '-tool', id || '-assistant', id, time_updated + 1, time_updated + 1,
 '{"type":"tool","tool":"bash","state":{"status":"running"}}' FROM session WHERE id IN ('working', 'stuck');
