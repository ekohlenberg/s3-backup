﻿using System;
using System.Collections.Generic;
using System.IO;
using System.Reflection;
namespace s3b
{
    class Program
    {
        static PersistBase persist = null;
        static Model cmdParams = null;

        public class UsageException : Exception
        {
            public UsageException(string message) : base(message + "\ns3b <backup_folder> <s3_bucket>\n")
            {
            }
        }

        
        static int Main(string[] args)
        {
            int retcode = 0;
            persist = new SqlitePersist(new s3bSqliteTemplate());
            Logger.Persist = persist;

            

            try
            {
                Logger.info("starting...");
                persist.execCmd("delete from message_log");

                BackupSet bset = parse(args);

                load(bset);

                init(bset);

                folders(bset);

                files(bset);

                newer(bset);

                process(bset);

                Logger.info("complete.");

            }
            catch( Exception e)
            {
                Logger.error(e);
                retcode = 1;
            }

            return retcode;
        }

        static BackupSet parse(string[] args)
        {
            if (args.Length != 2) throw new UsageException("Incorrect number of arguments.");
            BackupSet bset = new BackupSet();

            bset.root_folder_path = args[0];

            bset.root_folder_path = Path.GetFullPath(bset.root_folder_path);

            if (!Directory.Exists(bset.root_folder_path)) throw new UsageException("Folder " + bset.root_folder_path + " does not exist.");

            bset.upload_target = args[1];

            return bset;
        }

        private static Model getParameters(LocalFolder fldr)
        {
            if (cmdParams == null)
            {
                cmdParams = new Model();
                copySettings();
            }
            
            cmdParams["localfolder"] = fldr.folder_path;
            cmdParams["localfile"] = fldr.folder_path + "\\*";
            cmdParams["archive.name"] = fldr.getArchiveName();
            cmdParams["temp"] = Config.getString("s3b.temp");
            cmdParams["bucket"] = fldr.backupSet.upload_target;
            return cmdParams;
        }

        private static void copySettings()
        {
            Dictionary<string, string> settings = Config.getSettings();

            foreach (string k in settings.Keys)
            {
                cmdParams[k] = settings[k];
            }
        }

        // update all prev timestamp to the current timestemp (the last timestamp from the prev run)
        static void init(BackupSet bset)
        {
            
            string sql = @"update local_file set previous_update = current_update where folder_id in (
                           select fldr.id from local_folder fldr where fldr.backup_set_id =$(id))";

            persist.execCmd(bset, sql);
            
        }
        static void load(BackupSet bset)
        {
            Logger.info("loading backup set " + bset.root_folder_path);

            persist.put(bset, "root_folder_path");
        }

        static void folders(BackupSet bset)
        {
            string[] dirs = Directory.GetDirectories(bset.root_folder_path);

            Logger.info("loading folders for backup set " + bset.root_folder_path);

            LocalFolder fldr;

            foreach ( string d in dirs)
            {
                fldr = new LocalFolder();
                fldr.backup_set_id = bset.id;
                fldr.folder_path = d;
                persist.put(fldr, "folder_path");
                bset.localFolders.Add(fldr.id,fldr);
            }

            fldr = new LocalFolder();
            fldr.backup_set_id = bset.id;
            fldr.folder_path = bset.root_folder_path;
            fldr.recurse = false;
            persist.put(fldr, "folder_path");
            bset.localFolders.Add(fldr.id, fldr);
        }

        delegate void FileCallback(string filename);

        // put all files into the database recursively
        static void files(BackupSet bset)
        { 
            foreach(LocalFolder fldr in bset.localFolders.Values)
            {
                Logger.info("loading files for folder " + fldr.folder_path);

                FileCallback dcb = (filename) =>
                {
                    FileInfo fi = new FileInfo(filename);
                    LocalFile f = new LocalFile();

                    f.full_path = fi.FullName;
                    f.current_update = fi.LastWriteTime;
                    f.folder_id = fldr.id;

                    persist.put(f, "full_path");
                    fldr.files.Add(f);
                };

                if (fldr.recurse) dirSearch(fldr.folder_path, dcb);

                try
                {
                    foreach (string f in Directory.GetFiles(fldr.folder_path))
                    {
                        dcb(f);
                    }
                }
                catch(Exception x)
                {
                    Logger.error(x);
                }
            }
        }

        private static void dirSearch(string folder_path, FileCallback dcb)
        {
            try
            {
                foreach (string d in Directory.GetDirectories(folder_path))
                {

                    dirSearch(d, dcb);
                }

                foreach (string f in Directory.GetFiles(folder_path))
                {
                    dcb(f);
                }
            }
            catch (Exception x)
            {
                Logger.error(x);
            }
        }

        // select all the parent folders where there exists any file where the current timestamp is newer than the prev timestamp
        // default null prev timestmp to 1/1/1900 
        // insert found parent folders into a work list
        static void newer(BackupSet bset)
        {
           /* string sql = @"select distinct fldr.* from local_file f
                            inner join local_folder fldr on
	                            fldr.id = f.folder_id 
                            where
	                            fldr.backup_set_id=$(id) and
	                            (f.current_update > isnull( f.previous_update, convert( datetime, '1900-01-01', 102)) or
								(fldr.stage + '.' + fldr.status <> 'upload.complete'))
                            ";
           */
            PersistBase.SelectCallback scb = (rdr) =>
            {
                long id = Convert.ToInt64(rdr["id"]);

                if (bset.localFolders.ContainsKey(id))
                {
                    LocalFolder fldr = bset.localFolders[id];
                    Logger.debug("adding newer folder: " + fldr.folder_path);
                    bset.workFolders.Add(fldr);
                    fldr.backupSet = bset;
                    updateStatus(fldr, "new", "none");
                }
                else
                {
                    LocalFolder fldr = new LocalFolder();
                    persist.autoAssign(rdr, fldr);
                    bset.workFolders.Add(fldr);
                    bset.localFolders.Add(id, fldr);
                    fldr.backupSet = bset;
                    Logger.error(fldr.folder_path + " not found in dir listing.");
                }
            };

            persist.query(scb, "newer", bset);
        }

        static void process(BackupSet bset)
        {

            foreach ( LocalFolder fldr in bset.workFolders)
           // LocalFolder fldr = bset.workFolders[0];
            {
                if (fldr.files.Count > 0)
                {
                    int stageCode = fldr.getStageCode();

                    Logger.info("processing: " + fldr.folder_path);
                    if ((stageCode & (int)LocalFolder.stages.archiveStage) == (int)LocalFolder.stages.archiveStage)
                        archive(fldr);

                    if ((stageCode & (int)LocalFolder.stages.compressStage) == (int)LocalFolder.stages.compressStage)
                        compress(fldr);

                    if ((stageCode & (int)LocalFolder.stages.encryptStage) == (int)LocalFolder.stages.encryptStage)
                        encrypt(fldr);
                    
                    
                    if ((stageCode & (int)LocalFolder.stages.uploadStage) == (int)LocalFolder.stages.uploadStage)
                        upload(fldr);
                    
                    if ((stageCode & (int)LocalFolder.stages.cleanStage) == (int)LocalFolder.stages.cleanStage)
                        clean(fldr);
                    
                }

            }
        }

        private static void archive(LocalFolder fldr)
        {

            if (!isStepEnabled("archive")) return;

            Logger.info("archiving: " + fldr.folder_path);

            updateStatus(fldr, MethodBase.GetCurrentMethod().Name, "in_progess");
            Model cmdParams = getParameters(fldr);

            int retCode = 0;

                       
            if (fldr.recurse)
            {
                cmdParams["localobject"] = fldr.folder_path;
                retCode = exec("archive.command", "archive.args", cmdParams);
            }
            else
            {
                foreach (LocalFile f in fldr.files)
                {
                    f.getStdParams(cmdParams);
                    cmdParams["localfile"] = f.full_path;
                    cmdParams["localobject"] = f.full_path;

                    int execCode = exec("archive.command", "archive.args", cmdParams);
                    if (execCode == 1) retCode = 1;
                }
            }
           

            string completion = "complete";
            if (retCode != 0) completion = "error";

            updateStatus(fldr, MethodBase.GetCurrentMethod().Name, completion);
        }

        

        static void compress(LocalFolder fldr)
        {
            if (!isStepEnabled("compress")) return;

            Logger.info("compressing: " + fldr.folder_path);

            updateStatus(fldr, MethodBase.GetCurrentMethod().Name, "in_progess");
            Model cmdParams = getParameters(fldr);

            int retCode = exec("compress.command", "compress.args", cmdParams);
            string completion = "complete";
            if (retCode != 0) completion = "error";

            

            updateStatus(fldr, MethodBase.GetCurrentMethod().Name, completion);

        }

        static void encrypt(LocalFolder fldr)
        {
            if (!isStepEnabled("encrypt")) return;

            Logger.info("encrypting: " + fldr.folder_path);

            updateStatus(fldr, MethodBase.GetCurrentMethod().Name, "in_progess");

            Model cmdParams = getParameters(fldr);

            string passfile = System.Environment.GetEnvironmentVariable("S3B-PASSFILE");
            cmdParams["passfile"] = passfile;

            int retCode = exec("encrypt.command", "encrypt.args", cmdParams);
            string completion = "complete";
            if (retCode == 0)
            {
                Template t = new Template(cmdParams["encrypt.clean"].ToString());
                FileInfo fi = new FileInfo(t.eval(cmdParams));

                t.setScript(cmdParams["encrypt.target"].ToString());

                fldr.encrypted_file_name = t.eval(cmdParams);
                fldr.encrypted_file_size = fi.Length;

                persist.update(fldr);
            }
            else
            {
                completion = "error";
            }

            updateStatus(fldr, MethodBase.GetCurrentMethod().Name, completion);
        }

        static void upload(LocalFolder fldr)
        {
            if (!isStepEnabled("upload")) return;

            Logger.info("uploading: " + fldr.folder_path);

            updateStatus(fldr, MethodBase.GetCurrentMethod().Name, "in_progess");

            Model cmdParams = getParameters(fldr);

            //cmdParams["uploadtarget"] = Config.getString("upload.target");
            cmdParams["uploadtarget"] = fldr.backupSet.upload_target;

            string completion = "complete";
            if (exec("upload.command", "upload.args", cmdParams) != 0) completion = "error";
            
            updateStatus(fldr, MethodBase.GetCurrentMethod().Name, completion);
        }

        

        static int exec(string configCmdName, string configArgName, Model cmdParams)
        {
            string cmd = Config.getString(configCmdName);
            string args = Config.getString(configArgName);
            ProcExec pe = new ProcExec(cmd, args);


            return pe.run(cmdParams);
        }


        static void clean( LocalFolder fldr)
        {
            Logger.info("cleaning temp files: " + fldr.folder_path);

            Model cmdParams = getParameters(fldr);
            
           //  clean("archive.clean", cmdParams); // not necessary as gzip removes source fuke

            
            clean("compress.clean", cmdParams);
            clean("encrypt.clean", cmdParams);
            
            
            string completion = "complete";
            
            updateStatus(fldr, MethodBase.GetCurrentMethod().Name, completion);
            
        }
        static void clean(string configName, Model cmdParams)
        {
            
            string filename = Config.getString(configName);

            Template t = new Template(filename);

            filename = t.eval(cmdParams);

            if (File.Exists(filename))
            {
                Logger.info("cleaning " + filename);
                File.Delete(filename);
            }
            else
            {
                Logger.info("unable to clean " + filename);
            }
            
        }

        static void updateStatus( LocalFolder fldr, string stage, string status)
        {
            fldr.stage = stage;
            fldr.status = status;
            persist.update(fldr);
        }

        static  bool isStepEnabled(string stepName)
        {
            bool result = false;

            int enabled = Config.getInt(stepName + ".enabled");

            if (enabled == 1) result = true;

            return result;
        }
    }
}
