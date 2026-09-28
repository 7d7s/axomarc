//! Composition root module registry — the ONLY place modules are wired (D-43).
//! Adding a module = one Arc in this vec + its task row in 07-EXECUTION-PLAN.
use std::sync::Arc;

use kernel_runtime::HttpModule;

pub fn all_modules() -> Vec<Arc<dyn HttpModule>> {
    vec![
        Arc::new(mod_platform::PlatformModule),
        Arc::new(mod_org::OrgModule),
        Arc::new(mod_clinic::ClinicModule),
        Arc::new(mod_doctor::DoctorModule),
        Arc::new(mod_patient::PatientModule),
        Arc::new(mod_appointment::AppointmentModule),
        Arc::new(mod_rx::RxModule),
        Arc::new(mod_billing::BillingModule),
        Arc::new(mod_inventory::InventoryModule),
        Arc::new(mod_notifycfg::NotifyCfgModule),
        Arc::new(mod_reports::ReportsModule),
    ]
}