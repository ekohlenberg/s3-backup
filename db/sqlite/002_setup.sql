drop table local_file;

create table backup_log
(
id integer primary key autoincrement,
backup_time datetime,
last_write_time datetime,
last_upload_time datetime,
hostname text,
username text,
bucket text,
parent_folder text,
file_path text
);

create table app_version
(
id integer primary key autoincrement,
app_version int
);

insert into app_version (app_version) values (1);
update app_version set app_version = 2; 
