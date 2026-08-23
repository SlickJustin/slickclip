use uuid::Uuid;

use super::models::{LibrarySummary, StorageCleanupCandidate, StorageCleanupPreviewResponse};

pub const MIN_QUOTA_BYTES: u64 = 1_073_741_824;
pub const MAX_QUOTA_BYTES: u64 = 10 * 1_099_511_627_776;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StoredCleanupPlan {
    pub plan_id: String,
    pub quota_bytes: u64,
    pub total_size_bytes: u64,
    pub candidates: Vec<(String, u64)>,
}

pub fn build_cleanup_preview(
    quota_bytes: u64,
    summary: &LibrarySummary,
    available: Vec<StorageCleanupCandidate>,
) -> Result<(StorageCleanupPreviewResponse, Option<StoredCleanupPlan>), String> {
    if !(MIN_QUOTA_BYTES..=MAX_QUOTA_BYTES).contains(&quota_bytes) {
        return Err("Storage quota must be between 1 GB and 10 TB.".into());
    }
    let bytes_over_quota = summary.total_size_bytes.saturating_sub(quota_bytes);
    let mut planned_reclaim_bytes = 0_u64;
    let candidates = if bytes_over_quota == 0 {
        Vec::new()
    } else {
        available
            .into_iter()
            .take_while(|candidate| {
                if planned_reclaim_bytes >= bytes_over_quota {
                    return false;
                }
                planned_reclaim_bytes =
                    planned_reclaim_bytes.saturating_add(candidate.file_size_bytes);
                true
            })
            .collect::<Vec<_>>()
    };
    let can_meet_quota = planned_reclaim_bytes >= bytes_over_quota;
    let remaining_size_bytes = summary
        .total_size_bytes
        .saturating_sub(planned_reclaim_bytes);
    let plan = (!candidates.is_empty()).then(|| StoredCleanupPlan {
        plan_id: Uuid::new_v4().to_string(),
        quota_bytes,
        total_size_bytes: summary.total_size_bytes,
        candidates: candidates
            .iter()
            .map(|candidate| (candidate.clip_id.clone(), candidate.file_size_bytes))
            .collect(),
    });
    let response = StorageCleanupPreviewResponse {
        success: true,
        plan_id: plan.as_ref().map(|value| value.plan_id.clone()),
        quota_bytes,
        total_size_bytes: summary.total_size_bytes,
        bytes_over_quota,
        planned_reclaim_bytes,
        remaining_size_bytes,
        protected_count: summary.protected_count,
        protected_size_bytes: summary.protected_size_bytes,
        can_meet_quota,
        candidates,
        error_message: None,
    };
    Ok((response, plan))
}

pub fn same_cleanup_scope(left: &StoredCleanupPlan, right: &StoredCleanupPlan) -> bool {
    left.quota_bytes == right.quota_bytes
        && left.total_size_bytes == right.total_size_bytes
        && left.candidates == right.candidates
}

#[cfg(test)]
mod tests {
    use super::*;

    fn summary(total: u64, protected: u64) -> LibrarySummary {
        LibrarySummary {
            clip_count: 4,
            total_size_bytes: total,
            favorites_count: 1,
            protected_count: u64::from(protected > 0),
            protected_size_bytes: protected,
            collections_count: 0,
        }
    }

    fn candidate(id: &str, created_at_ms: i64, size: u64) -> StorageCleanupCandidate {
        StorageCleanupCandidate {
            clip_id: id.into(),
            display_name: id.into(),
            created_at_ms,
            file_size_bytes: size,
        }
    }

    #[test]
    fn cleanup_uses_oldest_unprotected_until_quota_is_met() {
        let gib = MIN_QUOTA_BYTES;
        let (preview, plan) = build_cleanup_preview(
            gib,
            &summary(gib + 250, 400),
            vec![
                candidate("oldest", 1, 100),
                candidate("middle", 2, 175),
                candidate("newest", 3, 900),
            ],
        )
        .unwrap();
        assert!(preview.can_meet_quota);
        assert_eq!(
            preview
                .candidates
                .iter()
                .map(|value| value.clip_id.as_str())
                .collect::<Vec<_>>(),
            vec!["oldest", "middle"]
        );
        assert_eq!(preview.planned_reclaim_bytes, 275);
        assert_eq!(
            plan.unwrap().candidates,
            vec![("oldest".into(), 100), ("middle".into(), 175)]
        );
    }

    #[test]
    fn protected_capacity_can_block_quota_without_selecting_protected_clips() {
        let gib = MIN_QUOTA_BYTES;
        let (preview, _) = build_cleanup_preview(
            gib,
            &summary(gib + 500, gib + 300),
            vec![candidate("only-unprotected", 1, 200)],
        )
        .unwrap();
        assert!(!preview.can_meet_quota);
        assert_eq!(preview.candidates.len(), 1);
        assert_eq!(preview.remaining_size_bytes, gib + 300);
    }

    #[test]
    fn cleanup_scope_detects_library_or_quota_changes() {
        let first = StoredCleanupPlan {
            plan_id: "one".into(),
            quota_bytes: MIN_QUOTA_BYTES,
            total_size_bytes: MIN_QUOTA_BYTES + 10,
            candidates: vec![("a".into(), 10)],
        };
        let same = StoredCleanupPlan {
            plan_id: "two".into(),
            ..first.clone()
        };
        let changed = StoredCleanupPlan {
            plan_id: "three".into(),
            candidates: vec![("b".into(), 10)],
            ..first.clone()
        };
        let larger_library = StoredCleanupPlan {
            plan_id: "four".into(),
            total_size_bytes: first.total_size_bytes + 1,
            ..first.clone()
        };
        assert!(same_cleanup_scope(&first, &same));
        assert!(!same_cleanup_scope(&first, &changed));
        assert!(!same_cleanup_scope(&first, &larger_library));
    }
}
