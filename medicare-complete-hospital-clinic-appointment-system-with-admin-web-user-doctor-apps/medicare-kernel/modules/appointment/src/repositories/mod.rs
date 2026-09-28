//! All SQL lives here (10-GUIDELINES §3.4). Runtime queries in the skeleton;
//! compile-time checked query! macros replace them once CI provisions the DB.
use kernel_core::SlotId;
use kernel_db::{DbError, Tx};
use sqlx::Row;
use time::OffsetDateTime;
use uuid::Uuid;

use crate::models::Slot;

pub async fn lock_open_slot(tx: &mut Tx<'_>, slot_id: SlotId) -> Result<Slot, DbError> {
    // FOR UPDATE is mandatory for contention rows (APT-02 / DOC-03).
    let row = sqlx::query(
        r"SELECT s.id, s.node_id, n.path, s.doctor_user_id, s.starts_at, s.state
          FROM slots s
          LEFT JOIN org_nodes n ON n.id = s.node_id
          WHERE s.id = $1
          FOR UPDATE OF s",
    )
    .bind(slot_id.0)
    .fetch_optional(tx.executor())
    .await?
    .ok_or(DbError::Sqlx(sqlx::Error::RowNotFound))?;

    Ok(Slot {
        id: SlotId::from(row.get::<Uuid, _>("id")),
        node_id: row.get::<Uuid, _>("node_id").into(),
        node_path: row.get::<Option<String>, _>("path").unwrap_or_default(),
        doctor_user_id: row.get("doctor_user_id"),
        starts_at: row.get::<OffsetDateTime, _>("starts_at"),
        state: row.get("state"),
    })
}

pub async fn insert_appointment(tx: &mut Tx<'_>, a: &crate::models::Appointment) -> Result<(), DbError> {
    sqlx::query(
        r"INSERT INTO appointments
            (id, tenant_id, owner_node, slot_id, patient_kind, patient_id,
             doctor_user_id, starts_at, status, version)
          VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,1)",
    )
    .bind(*a.id.as_uuid())
    .bind(a.tenant_id.0)
    .bind(a.owner_node.0)
    .bind(a.slot_id.0)
    .bind(&a.patient_kind)
    .bind(a.patient_id)
    .bind(a.doctor_user_id)
    .bind(a.starts_at)
    .bind(a.status)
    .execute(tx.executor())
    .await?;
    Ok(())
}

pub async fn mark_slot_booked(tx: &mut Tx<'_>, slot_id: SlotId, appointment_id: kernel_core::AppointmentId) -> Result<(), DbError> {
    sqlx::query("UPDATE slots SET state = 'booked', appointment_id = $2 WHERE id = $1 AND state = 'open'")
        .bind(slot_id.0)
        .bind(*appointment_id.as_uuid())
        .execute(tx.executor())
        .await?;
    Ok(())
}