-- WL-FUNC-033 leftover — drop the retired FDO notifications queue.
-- Created by 0002_settings_session.sql; notifications_server.rs and
-- notification_relay.rs have been gone since BUS-4.2. SQLite drops
-- indexes with the table.
DROP TABLE IF EXISTS notifications;
