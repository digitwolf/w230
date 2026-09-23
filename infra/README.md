# OTA endpoint (AWS)

`ota-stack.yaml` creates:

- a private, versioned, encrypted S3 bucket (`BucketName`);
- a CloudFront distribution with origin access control reading
  `<Prefix>/*` from it, HTTPS only, GET/HEAD, CachingOptimized;
- the bucket policy that allows only that distribution.

Deploy (authenticated AWS CLI, any region — CloudFront is global):

```sh
aws cloudformation deploy --region us-west-2 --stack-name w230-ota \
  --template-file infra/ota-stack.yaml \
  --parameter-overrides BucketName=w230-firmware-<account-id> Prefix=w230
aws cloudformation describe-stacks --region us-west-2 --stack-name w230-ota \
  --query 'Stacks[0].Outputs' --output table
```

Then:

1. put the `ManifestUrl` output into `OTA_MANIFEST_URL` in
   `firmware/esp32/src/main.rs` (or build with `W230_OTA_MANIFEST_URL=…`);
2. publish with `OTA_BUCKET=<BucketName> OTA_BASE_URL=https://<DistributionDomain>/w230 OTA_DISTRIBUTION_ID=<DistributionId> firmware/scripts/release.sh`.

Cost: cents per month at hobby volumes (S3 storage + CloudFront egress of a
~1.8 MB image per update). Objects are cached with `max-age=60` (manifest)
and `immutable` (images), set by `release.sh`; `OTA_DISTRIBUTION_ID`
invalidates the manifest path on publish so a release is visible at once.

Direct S3 alternative: making `<Prefix>/*` public-read and using
`https://<bucket>.s3.<region>.amazonaws.com/w230/manifest.json` also works
with the firmware's certificate bundle; the template avoids a public bucket.
