drop table backup_set;
drop table local_folder;
drop table local_file;
drop table message_log;


CREATE SEQUENCE object_identity as bigint

create table backup_set
(
id bigint , 
root_folder_path varchar(1024),
upload_target varchar(1024)
)

create table local_folder
(
id bigint ,
backup_set_id int,
folder_path varchar(1024),
stage varchar(32),
status varchar(32),
last_error varchar(2048)
)

create table local_file
(
id bigint ,
folder_id int,
full_path varchar(1024),
exclude int,
current_update datetime,
previous_update datetime
)

create table message_log
(
id bigint,
event_time datetime,
log_type varchar(16),
msg varchar(1024)
)