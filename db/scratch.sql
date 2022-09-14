delete from backup_set


select * from backup_set
insert into backup_set ( root_folder_path ) values ('C:\backup\PK2\data\src\softlap1\Personal')


update local_file set previous_update=current_update where folder_id in (
select lfldr.id from local_folder lfdr where lfdr.backup_set_id=$(bset.id))
