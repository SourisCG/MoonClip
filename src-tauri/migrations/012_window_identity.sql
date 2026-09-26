-- Registered games now match by real window identity instead of a process
-- rule. The title/app id (KDE) or window target (Windows) is learned from the
-- picker pick and stored per input.
ALTER TABLE custom_apps ADD COLUMN window_title TEXT;
ALTER TABLE custom_apps ADD COLUMN window_app_id TEXT;

-- Stale window rows from the process-rule era have no usable identity: the
-- engine would render them as black captures and the checker would never see
-- them. Drop them; the user re-registers once with the picker.
DELETE FROM custom_apps WHERE input_kind = 'window';
