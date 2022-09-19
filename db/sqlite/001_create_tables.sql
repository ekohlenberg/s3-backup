drop table backup_set;
drop table local_folder;
drop table local_file;
drop table message_log;




create table backup_set
(
id integer primary key autoincrement, 
root_folder_path text,
upload_target text
);

create table local_folder
(
id integer primary key autoincrement,
backup_set_id integer,
folder_path text,
stage text,
status text,
last_error text,
encrypted_file_name text,
encrypted_file_size integer
);

create table local_file
(
id integer primary key autoincrement,
folder_id integer,
full_path text,
exclude integer,
current_update datetime,
previous_update datetime
);

create table message_log
(
id integer primary key autoincrement,
event_time datetime,
log_type text,
msg text
);
