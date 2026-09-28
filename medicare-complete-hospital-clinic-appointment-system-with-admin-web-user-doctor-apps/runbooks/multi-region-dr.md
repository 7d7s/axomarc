# Runbook: Multi-Region DR (`multi-region-dr`)

> **Module owner:** Platform guild · **Last tested:** 2026-06-05 · **Drill cadence:** quarterly
> **On-call PagerDuty service:** `medicare-dr-prod` (only pages for actual region outage)
> **Datadog dashboard:** https://app.datadoghq.com/dashboard/medicare-dr
> **Sentry project:** N/A (DR, not runtime)

## 1. When to use

This runbook is the **DR playbook**. Page this runbook when the **primary region is down** or when a **DR drill is scheduled**.

- **PagerDuty alert:** `medicare-primary-region-down` (primary region health check fails for > 2 min), `medicare-rpo-violation` (replication lag > 5 min for > 5 min), `medicare-rto-violation` (failover took > 1 h).
- **Manual trigger:** scheduled DR drill (quarterly), or a planned failover for a primary-region maintenance window.

## 2. Detection

- **Datadog:** **DR** dashboard panels:
  - `dr.primary_region.up` (target: 1)
  - `dr.secondary_region.up` (target: 1, standby)
  - `dr.replication.lag_seconds` (target: ≤ 300 s; this is the RPO)
  - `dr.replication.lag_bytes` (target: ≤ 100 MB)
  - `dr.dns.health_check.failure_rate` (target: 0%)
- **Sentry:** N/A (DR is a platform-level concern, not a per-module concern).
- **Manual:** the platform on-call gets a Slack alert from the AWS Route 53 health check (`medicare-primary-health`).

## 3. Diagnosis

1. **Confirm the primary region is actually down** (not a monitoring false positive).
   ```bash
   aws route53 get-health-check-status --health-check-id med-primary | jq .
   ```
   If the primary health check fails from 3+ AWS regions, the region is down.
2. **Check the replication lag.**
   ```bash
   aws rds describe-db-instances --db-instance-identifier medicare-primary-replica --query 'DBInstances[0].ReplicaLag'
   ```
3. **Check the secondary region.**
   ```bash
   aws ec2 describe-instances --region us-west-2 --filters "Name=tag:medicare-role,Values=dr-standby" --query 'Reservations[].Instances[].State.Name'
   ```
4. **Check the S3 cross-region replication lag.**
   ```bash
   aws s3api get-bucket-replication --bucket medicare-primary-uploads --query 'Rules[0].Status'
   ```
5. **Determine the failover scope.** Is it:
   - DB only? (RDS failover, ~5 min)
   - API only? (Route 53 + ALB failover, ~2 min)
   - Full region? (RDS + API + workers + S3, ~15 min)

## 4. Mitigation (the actual DR failover)

> **WARNING:** This is a real failover. After step 3, the secondary region becomes primary; you cannot easily roll back. Communicate in `#dr-failover-{date}` before each step.

### Pre-flight (T+0 to T+5 min)
1. **Page the platform lead.** PagerDuty: `medicare-platform-lead`.
2. **Page the CTO** (or delegate). This is a company-level incident.
3. **Open the incident channel:** `#dr-failover-{date}`.
4. **Notify customer support:** "We are experiencing a regional outage; we are failing over to our DR region. ETA: 15-30 min."

### Failover (T+5 to T+15 min)
5. **Promote the RDS read replica** in the secondary region:
   ```bash
   aws rds promote-read-replica --db-instance-identifier medicare-secondary-replica --region us-west-2
   ```
   This takes ~5 min. The replica becomes the new primary.
6. **Update the Route 53 record** to point to the secondary region's ALB:
   ```bash
   aws route53 change-resource-record-sets --hosted-zone-id Z_MEDICARE --change-batch '{
     "Changes": [{
       "Action": "UPSERT",
       "ResourceRecordSet": {
         "Name": "api.medicare.app",
         "Type": "A",
         "AliasTarget": {
           "HostedZoneId": "Z_SECONDARY",
           "DNSName": "medicare-secondary-alb.us-west-2.elb.amazonaws.com",
           "EvaluateTargetHealth": true
         }
       }
     }]
   }'
   ```
7. **Scale up the secondary region's workers** (queue, scheduler, reverb):
   ```bash
   kubectl scale deployment/medicare-worker --replicas=20 -n medicare --context=us-west-2
   kubectl scale deployment/medicare-scheduler --replicas=2 -n medicare --context=us-west-2
   kubectl scale deployment/medicare-reverb --replicas=4 -n medicare --context=us-west-2
   ```
8. **Verify the secondary region is serving traffic:**
   ```bash
   curl -s -o /dev/null -w "%{http_code}" https://api.medicare.app/health
   # Expect 200
   ```

### Post-failover (T+15 to T+30 min)
9. **Monitor the secondary region** for 15 min: error rate, latency, queue depth.
10. **Verify data integrity:** run `php artisan dr:replication-audit --from=primary --to=secondary --since=1h` to compare the last 1 h of writes.
11. **Notify customers:** "We have successfully failed over to our DR region. The platform is operational. No data loss."
12. **Update the status page:** https://status.medicare.app → "Operational" (with a post-mortem note).
13. **Open the post-mortem doc:** `post-mortem/{date}-dr-failover.md`.

### If the primary region comes back (within 24 h)
- **Do not automatically fail back.** Schedule a failback for the next maintenance window (24-48 h later) to avoid data loss.
- Coordinate with the customer support team; some customers may have started new sessions in the secondary region.

## 5. Resolution

1. The platform is operational in the secondary region.
2. The primary region is quarantined (read-only, no traffic).
3. The incident is contained; the post-mortem is in progress.
4. Schedule the failback (T+24-48 h).
5. The CTO signs off on the post-mortem.

## 6. Post-mortem template

```yaml
Incident: DR-2026-06-05-001
Type: Unplanned DR failover (primary region us-east-1 down due to AWS network incident)
Duration: 14:23 → 15:08 UTC (45 min end-to-end)
Detection: Datadog alert medicare-primary-region-down (P1, immediate page)
Mitigation time (T+0 to T+15): 18 min (faster than the 1 h RTO target)
Resolution time (T+15 to T+45): 30 min (data integrity audit + customer comms)
Customer impact: 12% of API requests during the 5-15 min failover window returned 5xx; 0 PHI exposure; 0 data loss
Root cause: 5-why:
  1. Why was the primary region down? — AWS us-east-1 network incident (provider issue).
  2. Why did the failover take 18 min instead of the 10 min target? — The RDS promotion took 8 min (longer than the 5 min estimate).
  3. Why was the RDS promotion slow? — The replica had a backlog of 2.3 GB of WAL records.
  4. Why was the WAL backlog so large? — A long-running transaction (a tenant's bulk report) was holding a lock.
  5. Why was the long-running transaction not killed? — The lock timeout was 1 h (too generous for a replica).
Action items:
  - owner: platform-eng | due: 2026-06-12 | item: Reduce the RDS replica lock timeout to 5 min
  - owner: platform-eng | due: 2026-06-19 | item: Add a pre-failover check that kills long-running transactions on the replica
  - owner: platform-eng | due: 2026-07-05 | item: Schedule a failback drill for T+48 h
  - owner: cto | due: 2026-07-12 | item: Sign off on the post-mortem
```
