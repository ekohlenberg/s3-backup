# s3-backup

Tool to backup and sync to an S3-compatible bucket.  Can be used to protect against ransomware.

- Archives and compresses immediate folder children
- Uses client-side encryption to encrypt folders locally
- Uploads compressed, encrypted folders to S3
- Detects changes to local files and uploads only the containing folder
- Reconciles differences between the backup_folder and corresponding objects.  Uploads the local copy when a difference exists.
- Uses a configuration file to control external dependencies

# Usage

s3b	-action backup -folder <*backup_folder*> -bucket <*s3_bucket*>
		-action restore -bucket <*s3_bucket*> [-object <*object*>]

- *action*				: can be **backup** or **restore**
- *backup_folder* : the folder to be backed up
- *bucket*				: the target S3 bucket

# Dependencies

1. s3b uses tar and gzip to archive and compress files.  Windows requires cygwin or other utilities.
1. s3b has been only tested with AWS S3.  It is currently configured the AWS command-line, though it should be possible to use other cloud providers' command line utilities.
1. SQLite is used for the database.  An optional database browser can be used to view the backup information.


