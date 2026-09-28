-- Two-plane DB roles (05-SECURITY-ISOLATION §7.2): app handlers use medicare_app
-- (RLS-bound, NO access to platform schema); provisioning/break-glass uses medicare_platform.
CREATE ROLE medicare_app LOGIN PASSWORD 'medicare_app';
CREATE ROLE medicare_platform LOGIN PASSWORD 'medicare_platform';
-- privileges are granted by migrations (0001_kernel.sql)